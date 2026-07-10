use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eseqlisp::frame::build_render_frame;
use eseqlisp::parser::{Expr, ExprKind, Expression, Parser, SpannedASTParser};

use super::*;

const DEFAULT_CAPTURE_WIDTH: u32 = 2000;
const DEFAULT_CAPTURE_HEIGHT: u32 = 600;
const DEFAULT_CAPTURE_BUFFER: &str = "*fx*";
const DEFAULT_CAPTURE_OUTPUT: &str = "/tmp/metal-seq-capture.png";

#[derive(Debug, Clone)]
pub(crate) struct CaptureArgs {
    script: PathBuf,
    buffer: String,
    track: usize,
    width: u32,
    height: u32,
    out: PathBuf,
}

impl CaptureArgs {
    pub(crate) fn parse_env() -> Result<Option<Self>, String> {
        let mut args = std::env::args().skip(1);
        let Some(command) = args.next() else {
            return Ok(None);
        };
        if command != "capture" {
            return Ok(None);
        }

        let cwd = std::env::current_dir()
            .map_err(|error| format!("failed to read the current directory: {error}"))?;
        let mut script = None;
        let mut buffer = DEFAULT_CAPTURE_BUFFER.to_string();
        let mut track = 0usize;
        let mut width = DEFAULT_CAPTURE_WIDTH;
        let mut height = DEFAULT_CAPTURE_HEIGHT;
        let mut out = PathBuf::from(DEFAULT_CAPTURE_OUTPUT);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--script" => script = Some(PathBuf::from(next_arg(&mut args, "--script")?)),
                "--buffer" => buffer = normalize_buffer_name(&next_arg(&mut args, "--buffer")?),
                "--track" => track = parse_usize_arg(&mut args, "--track")?,
                "--width" => width = parse_dimension_arg(&mut args, "--width")?,
                "--height" => height = parse_dimension_arg(&mut args, "--height")?,
                "--out" => out = PathBuf::from(next_arg(&mut args, "--out")?),
                "-h" | "--help" => return Err(Self::usage()),
                other => {
                    return Err(format!(
                        "unknown capture argument {other}\n{}",
                        Self::usage()
                    ))
                }
            }
        }

        let script = script.ok_or_else(Self::usage)?;
        let script = absolute_path(&cwd, script);
        let out = absolute_path(&cwd, out);
        Ok(Some(Self {
            script,
            buffer,
            track,
            width,
            height,
            out,
        }))
    }

    fn usage() -> String {
        "usage: metal_seq capture --script PATH [--buffer '*fx*'] [--track N] [--width PX] [--height PX] [--out PATH]"
            .to_string()
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_usize_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<usize, String> {
    let value = next_arg(args, flag)?;
    value
        .parse::<usize>()
        .map_err(|_| format!("{flag} expects a non-negative integer, got {value:?}"))
}

fn parse_dimension_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<u32, String> {
    let value = next_arg(args, flag)?;
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{flag} expects a positive integer, got {value:?}"))?;
    if parsed == 0 {
        return Err(format!("{flag} expects a positive integer"));
    }
    Ok(parsed)
}

fn normalize_buffer_name(name: &str) -> String {
    let name = name.trim();
    if name.starts_with('*') && name.ends_with('*') {
        name.to_string()
    } else {
        format!("*{name}*")
    }
}

fn absolute_path(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureTrackKind {
    Sampler,
    Instrument(String),
    Modulator,
    DrumRack,
    LayerRack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTrackSpec {
    kind: CaptureTrackKind,
    display_name: Option<String>,
    midi_fx: Vec<String>,
    audio_fx: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureProjectSpec {
    tracks: Vec<CaptureTrackSpec>,
}

struct ParsedCaptureScript {
    project: CaptureProjectSpec,
    executable_source: String,
}

fn parse_capture_script(path: &Path) -> Result<ParsedCaptureScript, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read capture script {}: {error}", path.display()))?;
    parse_capture_source(&source)
        .map_err(|error| format!("invalid capture script {}: {error}", path.display()))
}

fn parse_capture_source(source: &str) -> Result<ParsedCaptureScript, String> {
    let tokens = Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| format!("tokenization failed: {error:?}"))?;
    let expressions = SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("parse failed: {error:?}"))?;

    let project_forms = expressions
        .iter()
        .filter(|expression| expression_head(expression) == Some("capture-project"))
        .collect::<Vec<_>>();
    let project_form = match project_forms.as_slice() {
        [] => return Err("missing top-level (capture-project ...) form".to_string()),
        [form] => *form,
        _ => {
            return Err("capture scripts must contain exactly one capture-project form".to_string())
        }
    };
    let project = parse_capture_project(&project_form.to_legacy())?;

    let span = &project_form.origin.primary_span;
    let hidden = source
        .get(span.start_byte..span.end_byte)
        .ok_or_else(|| "capture-project source span is invalid".to_string())?
        .chars()
        .map(|ch| if ch == '\n' { '\n' } else { ' ' })
        .collect::<String>();
    let executable_source = format!(
        "{}{}{}",
        &source[..span.start_byte],
        hidden,
        &source[span.end_byte..]
    );
    Ok(ParsedCaptureScript {
        project,
        executable_source,
    })
}

fn expression_head(expression: &Expr) -> Option<&str> {
    let ExprKind::List(items) = &expression.kind else {
        return None;
    };
    let first = items.first()?;
    match &first.kind {
        ExprKind::Symbol(name) => Some(name.as_str()),
        _ => None,
    }
}

fn parse_capture_project(expression: &Expression) -> Result<CaptureProjectSpec, String> {
    let Expression::List(items) = expression else {
        return Err("capture-project must be a list".to_string());
    };
    let mut tracks = Vec::new();
    for (index, expression) in items.iter().skip(1).enumerate() {
        tracks.push(
            parse_capture_track(expression)
                .map_err(|error| format!("capture-project track {}: {error}", index + 1))?,
        );
    }
    Ok(CaptureProjectSpec { tracks })
}

fn parse_capture_track(expression: &Expression) -> Result<CaptureTrackSpec, String> {
    let Expression::List(items) = expression else {
        return Err(
            "expected (track :sampler), (track :instrument \"name\"), or another track form"
                .to_string(),
        );
    };
    if expression_name(items.first()) != Some("track") {
        return Err("project entries must begin with track".to_string());
    }
    let kind_name =
        expression_name(items.get(1)).ok_or_else(|| "track is missing its kind".to_string())?;
    let mut cursor = 2;
    let kind = match kind_name {
        "sampler" => CaptureTrackKind::Sampler,
        "instrument" => {
            let name = expression_string(items.get(cursor))
                .ok_or_else(|| "instrument tracks require a saved instrument name".to_string())?;
            cursor += 1;
            CaptureTrackKind::Instrument(name.to_string())
        }
        "modulator" => CaptureTrackKind::Modulator,
        "drum-rack" => CaptureTrackKind::DrumRack,
        "layer-rack" => CaptureTrackKind::LayerRack,
        other => return Err(format!("unsupported track kind {other:?}")),
    };

    let mut display_name = None;
    let mut midi_fx = Vec::new();
    let mut audio_fx = Vec::new();
    while cursor < items.len() {
        let option = expression_name(items.get(cursor))
            .ok_or_else(|| format!("expected a track option at item {}", cursor + 1))?;
        let value = items
            .get(cursor + 1)
            .ok_or_else(|| format!("track option :{option} requires a value"))?;
        match option {
            "name" => {
                display_name = Some(
                    expression_string(Some(value))
                        .ok_or_else(|| ":name expects a string".to_string())?
                        .to_string(),
                );
            }
            "midi-fx" => midi_fx = expression_string_list(value, ":midi-fx")?,
            "audio-fx" => audio_fx = expression_string_list(value, ":audio-fx")?,
            other => return Err(format!("unsupported track option :{other}")),
        }
        cursor += 2;
    }

    Ok(CaptureTrackSpec {
        kind,
        display_name,
        midi_fx,
        audio_fx,
    })
}

fn expression_name(expression: Option<&Expression>) -> Option<&str> {
    match expression? {
        Expression::Symbol(value) | Expression::Keyword(value) | Expression::String(value) => {
            Some(value.trim_start_matches(':'))
        }
        _ => None,
    }
}

fn expression_string(expression: Option<&Expression>) -> Option<&str> {
    match expression? {
        Expression::String(value) | Expression::Symbol(value) => Some(value),
        _ => None,
    }
}

fn expression_string_list(expression: &Expression, option: &str) -> Result<Vec<String>, String> {
    let items = match expression {
        Expression::List(items) | Expression::QuoteList(items) => items,
        _ => return Err(format!("{option} expects a list of names")),
    };
    items
        .iter()
        .map(|item| {
            expression_string(Some(item))
                .map(str::to_string)
                .ok_or_else(|| format!("{option} expects only string or symbol names"))
        })
        .collect()
}

fn apply_capture_project(app: &mut ui::App, project: &CaptureProjectSpec) -> Result<(), String> {
    for (spec_index, spec) in project.tracks.iter().enumerate() {
        let track = match &spec.kind {
            CaptureTrackKind::Sampler => app.graph_controller().add_blank_sampler_track(),
            CaptureTrackKind::Instrument(name) => app.add_saved_instrument_track_sync(name),
            CaptureTrackKind::Modulator => app.graph_controller().add_modulator_track(),
            CaptureTrackKind::DrumRack => app.graph_controller().add_empty_rack_track(),
            CaptureTrackKind::LayerRack => app.graph_controller().add_empty_layer_rack_track(),
        }
        .map_err(|error| format!("failed to create track {}: {error}", spec_index + 1))?;

        if let Some(name) = &spec.display_name {
            app.tracks[track] = name.clone();
        }
        for effect in &spec.midi_fx {
            app.add_midi_fx_to_track_sync(track, effect)
                .map_err(|error| {
                    format!(
                        "failed to add MIDI FX {effect:?} to track {}: {error}",
                        track + 1
                    )
                })?;
        }
        for effect in &spec.audio_fx {
            app.add_builtin_effect_sync(track, effect)
                .map_err(|error| {
                    format!(
                        "failed to add audio FX {effect:?} to track {}: {error}",
                        track + 1
                    )
                })?;
        }
    }
    Ok(())
}

struct HeadlessGraphGuard {
    graph: *mut sequencer::audiograph::LiveGraph,
}

impl Drop for HeadlessGraphGuard {
    fn drop(&mut self) {
        unsafe {
            sequencer::audiograph::clear_os_workgroup();
            sequencer::audiograph::engine_stop_workers();
            sequencer::audiograph::destroy_live_graph(self.graph);
        }
    }
}

pub(crate) fn run(args: CaptureArgs) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_capture_script(&args.script)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let engine = engine::init_headless_engine(44_100, 2)?;
    let state = Arc::clone(&engine.state);
    let graph = engine.lg_ptr.0;
    let _graph_guard = HeadlessGraphGuard { graph };
    let master_recorder = Arc::clone(&engine.master_recorder);
    let mut app = ui::App::new(
        Arc::clone(&state),
        engine.lg_ptr,
        engine.sample_rate,
        engine.buses,
        engine.master_recorder,
        engine.keyboard_tx,
    );
    apply_capture_project(&mut app, &parsed.project)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    if !app.tracks.is_empty() && args.track >= app.tracks.len() {
        return Err(format!(
            "capture track {} is out of range for a {}-track project",
            args.track,
            app.tracks.len()
        )
        .into());
    }

    let mut track_names = app.tracks.clone();
    let track_pan_ids = Arc::new(Mutex::new(
        app.graph
            .track_node_ids
            .iter()
            .map(|node| node.pan_id)
            .collect(),
    ));
    let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
    let bus_state = Arc::new(Mutex::new(app.buses.clone()));
    let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
    let current_track = Arc::new(AtomicUsize::new(args.track));
    let selected_tracks = Arc::new(Mutex::new(HashSet::new()));
    let track_groups = Arc::new(Mutex::new(app.groups.clone()));
    let selected_steps = Arc::new(Mutex::new(HashSet::new()));
    let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
        Arc::new(Mutex::new(BTreeSet::new()));
    let piano_roll_selection = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_move_state = Arc::new(Mutex::new(None));
    let recording = Arc::new(AtomicBool::new(false));
    let master_recording = Arc::new(AtomicBool::new(false));
    let record_armed = Arc::new(Mutex::new(vec![false; app.tracks.len()]));
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    let fx_epoch = Arc::new(AtomicUsize::new(0));
    let ui_invalidations = Arc::new(UiInvalidationQueue::new());
    let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
    let active_delete_target = Arc::new(Mutex::new(None));
    let active_delete_target_version = Arc::new(AtomicUsize::new(0));
    let auto_follow_override_until = Arc::new(Mutex::new(None::<Instant>));

    let RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names: _,
        sample_browser: _,
    } = init_runtime(
        &app,
        Arc::clone(&state),
        &track_names,
        Arc::clone(&track_pan_ids),
        Arc::clone(&track_collapsed),
        Arc::clone(&bus_state),
        Arc::clone(&bus_node_ids),
        Arc::clone(&current_track),
        selected_tracks,
        track_groups,
        Arc::clone(&selected_steps),
        Arc::clone(&piano_roll_selection),
        piano_roll_move_state,
        recording,
        master_recording,
        master_recorder,
        Arc::clone(&record_armed),
        Arc::clone(&ui_epoch),
        fx_epoch,
        ui_invalidations,
        expanded_step_projection,
        selected_neural_neurons,
        active_delete_target,
        active_delete_target_version,
        auto_follow_override_until,
        graph,
    );

    let mut editor = create_editor(runtime, &app)?;
    editor
        .runtime_mut()
        .eval_str("(def capture-after-sync () nil)")
        .map_err(|error| format!("failed to install capture hook: {error:?}"))?;
    editor
        .runtime_mut()
        .eval_source_at_path(args.script.clone(), &parsed.executable_source)
        .map_err(|error| format!("capture setup Lisp failed: {error:?}"))?;
    {
        let runtime = editor.runtime_mut();
        sync_project_state(runtime, &app);
        sync_track_topology_state(
            runtime,
            &app,
            &state,
            &mut track_names,
            args.track,
            &selected_steps,
            &piano_roll_selection,
            &accumulator_names,
            &record_armed,
            &vec![0.0; app.tracks.len()],
        );
        runtime.set_reactive(
            "SEQ",
            "selected-steps",
            build_selection_value(&selected_steps),
        );
        runtime.set_reactive(
            "SEQ",
            "bus-effects",
            build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
        );
        runtime.run_reactive_cycle();
    }
    editor.refresh_runtime_side_effects();
    editor
        .runtime_mut()
        .eval_str("(capture-after-sync)")
        .map_err(|error| format!("capture-after-sync failed: {error:?}"))?;
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let buffer_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == args.buffer)
        .map(|buffer| buffer.id)
        .ok_or_else(|| format!("capture buffer {:?} does not exist", args.buffer))?;
    editor.set_active_buffer(buffer_id);
    editor
        .runtime_mut()
        .eval_str("(delete-other-windows)")
        .map_err(|error| format!("failed to isolate capture buffer: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    editor.clear_minibuffer_message();

    let mut backend = create_capture_backend(&mut editor, args.width, args.height)?;
    let (cell_width, cell_height) = backend.cell_dimensions();
    let columns = ((args.width as f32 / cell_width).floor() as usize).clamp(1, u16::MAX as usize);
    let rows = ((args.height as f32 / cell_height).floor() as usize).clamp(1, u16::MAX as usize);
    editor.set_layout_viewport(columns as u16, rows as u16);
    editor.update_tile_rects(columns as u16, rows as u16);
    let frame = build_render_frame(&mut editor, columns, rows);

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    backend
        .render_frame_to_png(&frame, args.width, args.height, &args.out)
        .map_err(|_| "failed to render capture PNG")?;
    println!("{}", args.out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_tracks_and_preserves_non_project_source() {
        let source = r#"
            (capture-project
              (track :sampler :name "Drums" :midi-fx ("arp") :audio-fx '("filter"))
              (track :instrument "core/drift"))

            (def-process passthrough :run nil)
            (processes :track 0 (passthrough))
        "#;
        let parsed = parse_capture_source(source).expect("capture source should parse");
        assert_eq!(parsed.project.tracks.len(), 2);
        assert_eq!(
            parsed.project.tracks[0],
            CaptureTrackSpec {
                kind: CaptureTrackKind::Sampler,
                display_name: Some("Drums".to_string()),
                midi_fx: vec!["arp".to_string()],
                audio_fx: vec!["filter".to_string()],
            }
        );
        assert_eq!(
            parsed.project.tracks[1].kind,
            CaptureTrackKind::Instrument("core/drift".to_string())
        );
        assert!(!parsed.executable_source.contains("capture-project"));
        assert!(parsed.executable_source.contains("def-process passthrough"));
        assert!(parsed.executable_source.contains("processes :track 0"));
        assert_eq!(
            source.lines().count(),
            parsed.executable_source.lines().count(),
            "removing the declarative form should preserve diagnostic line numbers"
        );
    }

    #[test]
    fn capture_project_is_required_and_unique() {
        assert!(parse_capture_source("(def x 1)").is_err());
        assert!(parse_capture_source("(capture-project) (capture-project)").is_err());
    }

    #[test]
    fn normalizes_convenient_buffer_names() {
        assert_eq!(normalize_buffer_name("fx"), "*fx*");
        assert_eq!(normalize_buffer_name("*sequencer*"), "*sequencer*");
    }
}
