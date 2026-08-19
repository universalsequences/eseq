use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
                    ));
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
    LayerRack,
}

#[derive(Debug, Clone, PartialEq)]
struct CaptureTrackSpec {
    kind: CaptureTrackKind,
    display_name: Option<String>,
    solo: bool,
    num_steps: Option<usize>,
    /// `(step, transpose)` pairs authored via `:steps (0 (4 12) 8 ...)`;
    /// applied to the live pattern and persisted into the scene's pattern
    /// pool so pool-derived read surfaces (song lane previews) see them.
    steps: Vec<(usize, f32)>,
    samples: Vec<String>,
    midi_fx: Vec<String>,
    audio_fx: Vec<String>,
    rack_slot_audio_fx: Vec<String>,
    /// Open the Filter Table response editor on this track's Filter Table
    /// slot after effects install (eseq-dtx.8 visual review).
    filter_table_editor: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct CaptureProjectSpec {
    tracks: Vec<CaptureTrackSpec>,
    /// How many scenes the project should have. The headless capture project
    /// starts with one; `(scenes N)` clones the first N-1 more so a fixture
    /// can define a song that actually CHANGES scene.
    scenes: usize,
}

struct ParsedCaptureScript {
    project: CaptureProjectSpec,
    executable_source: String,
}

fn parse_capture_script(path: &Path) -> Result<ParsedCaptureScript, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read capture script {}: {error}", path.display()))?;
    let mut parsed = parse_capture_source(&source)
        .map_err(|error| format!("invalid capture script {}: {error}", path.display()))?;
    let cwd = std::env::current_dir()
        .map_err(|error| format!("failed to resolve capture working directory: {error}"))?;
    let script_dir = path.parent().unwrap_or_else(|| Path::new("."));
    for track in &mut parsed.project.tracks {
        for sample in &mut track.samples {
            let sample_path = Path::new(sample);
            *sample = absolute_path(&cwd, script_dir.join(sample_path))
                .to_string_lossy()
                .into_owned();
        }
    }
    Ok(parsed)
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
            return Err(
                "capture scripts must contain exactly one capture-project form".to_string(),
            );
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
    let mut scenes = 1usize;
    for (index, expression) in items.iter().skip(1).enumerate() {
        if expression_name(expression_head_item(expression)) == Some("scenes") {
            let Expression::List(items) = expression else {
                unreachable!("head matched, so this is a list");
            };
            scenes = items
                .get(1)
                .and_then(expression_usize)
                .filter(|count| *count >= 1)
                .ok_or_else(|| "(scenes N) expects a positive integer".to_string())?;
            continue;
        }
        tracks.push(
            parse_capture_track(expression)
                .map_err(|error| format!("capture-project track {}: {error}", index + 1))?,
        );
    }
    Ok(CaptureProjectSpec { tracks, scenes })
}

/// The head item of a list expression, for dispatching `capture-project`
/// entries that are not tracks.
fn expression_head_item(expression: &Expression) -> Option<&Expression> {
    match expression {
        Expression::List(items) => items.first(),
        _ => None,
    }
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
        "layer-rack" => CaptureTrackKind::LayerRack,
        other => return Err(format!("unsupported track kind {other:?}")),
    };

    let mut display_name = None;
    let mut solo = false;
    let mut num_steps = None;
    let mut steps = Vec::new();
    let mut samples = Vec::new();
    let mut midi_fx = Vec::new();
    let mut audio_fx = Vec::new();
    let mut rack_slot_audio_fx = Vec::new();
    let mut filter_table_editor = false;
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
            "solo" => {
                solo = expression_bool(value)
                    .ok_or_else(|| ":solo expects true or false".to_string())?;
            }
            "num-steps" => {
                let steps = expression_usize(value)
                    .ok_or_else(|| ":num-steps expects a positive integer".to_string())?;
                if !(1..=MAX_STEPS).contains(&steps) {
                    return Err(format!(
                        ":num-steps must be between 1 and {MAX_STEPS}, got {steps}"
                    ));
                }
                num_steps = Some(steps);
            }
            "steps" => steps = parse_capture_steps(value)?,
            "samples" => samples = expression_string_list(value, ":samples")?,
            "midi-fx" => midi_fx = expression_string_list(value, ":midi-fx")?,
            "audio-fx" => audio_fx = expression_string_list(value, ":audio-fx")?,
            "rack-slot-audio-fx" => {
                rack_slot_audio_fx = expression_string_list(value, ":rack-slot-audio-fx")?
            }
            "filter-table-editor" => {
                filter_table_editor = expression_bool(value)
                    .ok_or_else(|| ":filter-table-editor expects true or false".to_string())?;
            }
            other => return Err(format!("unsupported track option :{other}")),
        }
        cursor += 2;
    }

    if !samples.is_empty() && kind != CaptureTrackKind::LayerRack {
        return Err(":samples is only supported for :layer-rack tracks".to_string());
    }

    Ok(CaptureTrackSpec {
        kind,
        display_name,
        solo,
        num_steps,
        steps,
        samples,
        midi_fx,
        audio_fx,
        rack_slot_audio_fx,
        filter_table_editor,
    })
}

fn expression_step_index(expression: &Expression) -> Option<usize> {
    match expression {
        Expression::Number(value)
            if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
        {
            let step = usize::try_from(*value as u64).ok()?;
            (step < MAX_STEPS).then_some(step)
        }
        _ => None,
    }
}

fn expression_f32(expression: &Expression) -> Option<f32> {
    match expression {
        Expression::Number(value) if value.is_finite() => Some(*value as f32),
        _ => None,
    }
}

/// `:steps` entries are either a bare step index or a `(step transpose)` pair.
fn parse_capture_steps(value: &Expression) -> Result<Vec<(usize, f32)>, String> {
    let items = match value {
        Expression::List(items) | Expression::QuoteList(items) => items,
        _ => {
            return Err(
                ":steps expects a list of step indices or (step transpose) pairs".to_string()
            );
        }
    };
    items
        .iter()
        .map(|item| {
            if let Some(step) = expression_step_index(item) {
                return Ok((step, 0.0));
            }
            if let Expression::List(pair) | Expression::QuoteList(pair) = item {
                if let [step, transpose] = pair.as_slice() {
                    if let (Some(step), Some(transpose)) =
                        (expression_step_index(step), expression_f32(transpose))
                    {
                        return Ok((step, transpose));
                    }
                }
            }
            Err(":steps entries must be a step index or a (step transpose) pair".to_string())
        })
        .collect()
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

fn expression_usize(expression: &Expression) -> Option<usize> {
    match expression {
        Expression::Number(value) if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 => {
            usize::try_from(*value as u64).ok()
        }
        _ => None,
    }
}

fn expression_bool(expression: &Expression) -> Option<bool> {
    match expression_name(Some(expression))? {
        "true" => Some(true),
        "false" => Some(false),
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

fn apply_capture_project(app: &mut app::App, project: &CaptureProjectSpec) -> Result<(), String> {
    for (spec_index, spec) in project.tracks.iter().enumerate() {
        let track = match &spec.kind {
            CaptureTrackKind::Sampler => app.graph_controller().add_blank_sampler_track(),
            CaptureTrackKind::Instrument(name) => app.add_saved_instrument_track_sync(name),
            CaptureTrackKind::Modulator => app.graph_controller().add_modulator_track(),
            CaptureTrackKind::LayerRack => app.graph_controller().add_empty_layer_rack_track(),
        }
        .map_err(|error| format!("failed to create track {}: {error}", spec_index + 1))?;

        if let Some(name) = &spec.display_name {
            app.tracks[track] = name.clone();
        }
        app.state.pattern.track_params[track].set_solo(spec.solo);
        if let Some(num_steps) = spec.num_steps {
            app.state.pattern.track_params[track].set_num_steps(num_steps);
        }
        for &(step, transpose) in &spec.steps {
            app.state.pattern.patterns[track].set_step_active(step, true);
            app.state.pattern.step_data[track].set(step, StepParam::Transpose, transpose);
        }
        for sample in &spec.samples {
            let result = match spec.kind {
                CaptureTrackKind::LayerRack => app
                    .graph_controller()
                    .add_sampler_slot_to_rack(track, Path::new(sample)),
                _ => unreachable!("sample-bearing capture track kind was validated"),
            };
            result.map_err(|error| {
                format!(
                    "failed to add sample {sample:?} to rack track {}: {error}",
                    track + 1
                )
            })?;
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
        for effect in &spec.rack_slot_audio_fx {
            if spec.kind != CaptureTrackKind::LayerRack || spec.samples.is_empty() {
                return Err(format!(
                    "track {} needs a populated layer rack for :rack-slot-audio-fx",
                    spec_index + 1
                ));
            }
            app.add_builtin_rack_slot_effect_sync(track, 0, effect)
                .map_err(|error| {
                    format!(
                        "failed to add rack-slot Audio FX {effect:?} to track {}: {error}",
                        track + 1
                    )
                })?;
        }
        if spec.filter_table_editor {
            use sequencer::effects::filter_table_editor::{
                EditOp, EditorTarget, ParametricKind,
            };
            let slot = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| {
                    descs
                        .iter()
                        .position(|desc| desc.name == sequencer::effects::filter_table::NAME)
                })
                .ok_or_else(|| {
                    format!(
                        ":filter-table-editor on track {} needs \"Filter Table\" in :audio-fx",
                        track + 1
                    )
                })?;
            app.open_filter_table_editor(EditorTarget::Track { track, slot })
                .map_err(|error| format!("failed to open Filter Table editor: {error}"))?;
            // A visible session for review: one parametric node (renders as
            // the draggable band) over a gently tilted table, on frame 12.
            app.filter_table_editor_apply_op(
                EditOp::Tilt {
                    frame_start: 0,
                    frame_end: 63,
                    db_per_octave: -3.0,
                },
                false,
            )
            .map_err(|error| format!("failed to seed Filter Table editor op: {error}"))?;
            app.filter_table_editor_apply_op(
                EditOp::Parametric {
                    frame_start: 0,
                    frame_end: 63,
                    node: ParametricKind::Peak.default_node(),
                },
                false,
            )
            .map_err(|error| format!("failed to seed Filter Table editor node: {error}"))?;
            app.filter_table_editor_select_frame(12)
                .map_err(|error| format!("failed to select Filter Table editor frame: {error}"))?;
        }
    }
    // :steps write the live pattern; persist them into the scene's pattern
    // pool through the production scene-launch path (capture current
    // snapshot, save, relaunch the same scene) so pool-derived read surfaces
    // (song lane previews) observe them.
    if project.tracks.iter().any(|spec| !spec.steps.is_empty()) {
        let scene = app.state.current_scene_index();
        app.state
            .launch_scene(
                scene,
                app.tracks.len(),
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            )
            .ok_or_else(|| {
                format!("failed to persist :steps into scene {} pattern pool", scene + 1)
            })?;
    }
    // Extra scenes, cloned from the first exactly as the "clone-pattern" host
    // command does, so every scene has a full cell set for every track.
    for _ in 1..project.scenes {
        app.state.clone_pattern(
            app.tracks.len(),
            &app.graph.track_buffer_ids,
            &app.graph.track_sample_rates,
            &app.tracks,
            &app.graph.track_instrument_types,
        );
    }
    if project.scenes > 1 {
        app.state.launch_scene(
            0,
            app.tracks.len(),
            &app.graph.track_buffer_ids,
            &app.graph.track_sample_rates,
            &app.tracks,
            &app.graph.track_instrument_types,
        );
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

fn apply_capture_macro_host_commands(
    editor: &mut Editor,
    app: &mut app::App,
    state: &SequencerState,
    current_track: usize,
) -> Result<bool, String> {
    let mut applied = false;
    for command in editor.drain_host_commands() {
        let HostCommand::Custom { name, payload } = command else {
            continue;
        };
        // Song/arrangement editing primitives (def-song lowers to
        // arrangement-replace) so arrangement fixtures can commit a song
        // during capture setup.
        if let Some(result) = crate::host_commands::apply_song_edit_command(&name, &payload, app) {
            result.map_err(|error| format!("capture setup {name} failed: {error}"))?;
            applied = true;
            continue;
        }
        // Sound-palette open/close so fixtures can capture the palette modal.
        if let Some(result) =
            crate::host_commands::apply_sound_palette_view_command(&name, &payload, app)
        {
            result.map_err(|error| format!("capture setup {name} failed: {error}"))?;
            applied = true;
            continue;
        }
        match handle_macro_host_command(&name, &payload, app, state, current_track) {
            MacroHostCommandOutcome::Applied => applied = true,
            MacroHostCommandOutcome::Ignored => {
                return Err(format!(
                    "capture setup emitted invalid {name} payload: {payload:?}"
                ));
            }
            MacroHostCommandOutcome::NotMacro => {}
        }
    }
    Ok(applied)
}

/// Optional standalone delta-glyph gallery feed. Entries sharing an
/// instrument form a cohort; the first entry is its reference. Thin source
/// metadata degrades exactly as the spec requires: linear taper, one group.
fn publish_capture_sound_glyphs(editor: &mut Editor) -> Result<(), String> {
    use eseqlisp::vm::Value;
    use sequencer::delta_glyph::{
        DeltaGlyphCohort, IdentityBranch, ParamGroup, ParamKind, ParamSchema, ParamTaper,
    };

    let Ok(Some(Value::List(entries))) = editor.runtime_mut().eval_str("capture-sound-glyphs") else {
        return Ok(());
    };
    let get_string = |map: &std::collections::HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>, key: &str| {
        map.get(key).and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
    };
    struct Captured {
        key: String,
        instrument: String,
        schema: Vec<ParamSchema>,
        values: Vec<f32>,
        identity: Vec<IdentityBranch>,
    }
    let mut captured = Vec::new();
    for entry in &entries {
        let entry = entry.borrow();
        let Value::Map(map) = &*entry else { return Err("capture-sound-glyphs expects dict entries".to_string()) };
        let key = get_string(map, "key").ok_or("capture-sound-glyphs entry missing :key")?;
        let instrument = get_string(map, "instrument").ok_or("capture-sound-glyphs entry missing :instrument")?;
        let source = sequencer::lisp_host::load_instrument_source(&instrument)
            .map_err(|error| format!("capture-sound-glyphs: load {instrument}: {error}"))?;
        let specs = sequencer::sound_glyph::param_specs(&source);
        let mut schema = specs.iter().enumerate().map(|(order, (name, spec))| {
            let lower = name.to_ascii_lowercase();
            let group = if lower.contains("osc") || lower.contains("wave") || lower.contains("pitch") {
                ParamGroup::Osc
            } else if lower.contains("filter") || lower.contains("cutoff") || lower.contains("reson") {
                ParamGroup::Filter
            } else if lower.contains("env") || lower.contains("attack") || lower.contains("decay") || lower.contains("release") {
                ParamGroup::Env
            } else if lower.contains("mod") || lower.contains("lfo") {
                ParamGroup::Mod
            } else if lower.contains("delay") || lower.contains("reverb") || lower.contains("fx") {
                ParamGroup::Fx
            } else if lower.contains("level") || lower.contains("gain") || lower.contains("pan") || lower.contains("mix") {
                ParamGroup::Mix
            } else {
                ParamGroup::Other("other".to_string())
            };
            ParamSchema {
            id: name.clone(), kind: ParamKind::Continuous, range: (spec.min, spec.max),
            taper: ParamTaper::Linear, group,
            order, link: None, visible: true, audio: true, default: spec.default, weight: 1.0,
        }}).collect::<Vec<_>>();
        schema.sort_by(|a, b| a.id.cmp(&b.id));
        let mut values = schema.iter().map(|param| param.default).collect::<Vec<_>>();
        if let Some(params) = map.get("params") {
            if let Value::Map(params) = &*params.borrow() {
                for (name, value) in params {
                    if let (Some(index), Value::Number(number)) = (
                        schema.iter().position(|param| &param.id == name), &*value.borrow()
                    ) { values[index] = *number as f32; }
                }
            }
        }
        // Identity tier (spec §5.1a): an all-identical fixture cohort still
        // renders the instrument's AST silhouette instead of a void.
        let identity = sequencer::sound_glyph::identity_branches(
            &sequencer::sound_glyph::extract_skeleton(&source).skeleton,
        );
        captured.push(Captured { key, instrument, schema, values, identity });
    }
    for item in &captured {
        let cohort = captured.iter().filter(|other| other.instrument == item.instrument)
            .map(|other| other.values.clone()).collect::<Vec<_>>();
        let reference = cohort.first().cloned().unwrap_or_default();
        let delta =
            DeltaGlyphCohort::new_with_identity(&item.schema, &cohort, &reference, &item.identity)
                .build(&item.values, item.values == reference);
        eseqlisp::sound_glyph_data::publish_sound_glyph_frame(item.key.clone(), eseqlisp::sound_glyph_data::SoundGlyphFrame {
            revision: 1, cols: delta.cols, rows: delta.rows,
            substrate: delta.substrate,
            pieces: delta.pieces.into_iter().map(|piece| eseqlisp::sound_glyph_data::SoundGlyphPiece {
                slot: piece.slot, piece: piece.piece, hue: piece.hue,
                magnitude: piece.magnitude, mirror: piece.mirror, negative: piece.negative,
            }).collect(),
            anchor: delta.anchor,
            incompatible: false,
        });
    }
    Ok(())
}

/// Optional `(def capture-click-widgets (list "dropdown"))` in a capture
/// script: after the first layout, synthesize a left click on the center of
/// the first widget of each named type. This lets fixtures capture state that
/// only opens through input, such as a dropdown's menu overlay. Returns true
/// if any click was dispatched (the frame must then be rebuilt).
fn apply_capture_click_widgets(
    editor: &mut Editor,
    columns: usize,
    rows: usize,
) -> Result<bool, String> {
    fn find_widget_of_type<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget_of_type(child, widget_type))
    }

    let Ok(Some(eseqlisp::vm::Value::List(entries))) =
        editor.runtime_mut().eval_str("capture-click-widgets")
    else {
        return Ok(false);
    };
    let names = entries
        .iter()
        .map(|entry| match &*entry.borrow() {
            eseqlisp::vm::Value::String(name) => Ok(name.clone()),
            other => Err(format!(
                "capture-click-widgets expects widget-type strings, got {other:?}"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Ok(false);
    }
    for name in names {
        let layout = editor
            .widget_layout()
            .ok_or("capture-click-widgets requires a widget layout")?;
        let node = find_widget_of_type(&layout, &name)
            .ok_or_else(|| format!("capture-click-widgets: no {name:?} widget in the layout"))?;
        let col = node.rect.col + node.rect.width * 0.5;
        let row = node.rect.row + node.rect.height * 0.5;
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            editor.handle_mouse_precise(
                MouseEvent {
                    kind,
                    column: col.floor() as u16,
                    row: row.floor() as u16,
                    modifiers: KeyModifiers::NONE,
                },
                0,
                0,
                columns as u16,
                rows as u16,
                col,
                row,
            );
        }
        editor.refresh_runtime_side_effects();
    }
    Ok(true)
}

pub(crate) fn run(args: CaptureArgs) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_capture_script(&args.script)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let engine = engine::init_headless_engine(44_100, 2)?;
    let state = Arc::clone(&engine.state);
    let graph = engine.lg_ptr.0;
    let _graph_guard = HeadlessGraphGuard { graph };
    let master_recorder = Arc::clone(&engine.master_recorder);
    let mut app = app::App::new(
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
    let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
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
        piano_roll_clipboard: _,
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
        new_shared_piano_roll_focus(),
        recording,
        master_recording,
        master_recorder,
        Arc::clone(&record_armed),
        Arc::clone(&armed_rack),
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
    apply_capture_macro_host_commands(&mut editor, &mut app, &state, args.track)?;
    {
        let runtime = editor.runtime_mut();
        sync_project_state(runtime, &app);
        sync_macro_state(runtime, &app);
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
        sync_song_state(runtime, &app, &mut SongFrameState::default(), true);
        runtime.run_reactive_cycle();
    }
    editor.refresh_runtime_side_effects();
    editor
        .runtime_mut()
        .eval_str("(capture-after-sync)")
        .map_err(|error| format!("capture-after-sync failed: {error:?}"))?;
    if apply_capture_macro_host_commands(&mut editor, &mut app, &state, args.track)? {
        sync_macro_state(editor.runtime_mut(), &app);
        sync_song_state(
            editor.runtime_mut(),
            &app,
            &mut SongFrameState::default(),
            true,
        );
    }
    // Publish the sound-palette read surfaces so capture scripts can open the
    // palette modal via the real (seq-sound-palette-open ...) funnel.
    let _ = sync_sound_palette(
        editor.runtime_mut(),
        &app,
        &mut SoundPaletteFrameState::default(),
        true,
    );
    publish_capture_sound_glyphs(&mut editor)?;
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
    let mut frame = build_render_frame(&mut editor, columns, rows);
    if apply_capture_click_widgets(&mut editor, columns, rows)? {
        frame = build_render_frame(&mut editor, columns, rows);
    }

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
              (track :sampler :name "Drums" :solo true :num-steps 8 :midi-fx ("arp") :audio-fx '("filter"))
              (track :layer-rack :samples ("kick.wav" "snare.wav"))
              (track :instrument "core/drift"))

            (def-process passthrough :run nil)
            (processes :track 0 (passthrough))
        "#;
        let parsed = parse_capture_source(source).expect("capture source should parse");
        assert_eq!(parsed.project.tracks.len(), 3);
        assert_eq!(
            parsed.project.tracks[0],
            CaptureTrackSpec {
                kind: CaptureTrackKind::Sampler,
                display_name: Some("Drums".to_string()),
                solo: true,
                num_steps: Some(8),
                steps: vec![],
                samples: vec![],
                midi_fx: vec!["arp".to_string()],
                audio_fx: vec!["filter".to_string()],
                rack_slot_audio_fx: vec![],
                filter_table_editor: false,
            }
        );
        assert_eq!(
            parsed.project.tracks[1].samples,
            vec!["kick.wav".to_string(), "snare.wav".to_string()]
        );
        assert_eq!(
            parsed.project.tracks[2].kind,
            CaptureTrackKind::Instrument("core/drift".to_string())
        );
        assert_eq!(parsed.project.tracks[2].num_steps, None);
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
    fn capture_track_num_steps_rejects_out_of_range_values() {
        for source in [
            "(capture-project (track :sampler :num-steps 0))",
            "(capture-project (track :sampler :num-steps 8.5))",
            "(capture-project (track :sampler :num-steps 999))",
        ] {
            assert!(
                parse_capture_source(source).is_err(),
                "source should fail: {source}"
            );
        }
    }

    #[test]
    fn capture_track_samples_are_limited_to_racks() {
        assert!(
            parse_capture_source("(capture-project (track :sampler :samples (\"kick.wav\")))")
                .is_err()
        );
        assert!(parse_capture_source(
            "(capture-project (track :layer-rack :samples (\"kick.wav\")))"
        )
        .is_ok());
    }

    #[test]
    fn normalizes_convenient_buffer_names() {
        assert_eq!(normalize_buffer_name("fx"), "*fx*");
        assert_eq!(normalize_buffer_name("*sequencer*"), "*sequencer*");
    }
}
