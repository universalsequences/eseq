//! The shape lab (content/ui/modal/shape-lab.lisp): a buffer beside a modal
//! script that edits one finite-element solid's shape, material and mode
//! count with a live 3-D view, solves it in the background, and writes the
//! result back into the script.
//!
//! - `shape-lab-load` evaluates the script and hands the lab its solids
//!   (`m/solid` objects carry the spec they were solved from as `:fe-spec`).
//! - `shape-lab-solve` solves a spec on a worker thread into the cache
//!   `modal-solve` reads, so the draft's next evaluation finds it solved.
//!   One solve runs at a time: a newer request cancels the running one and
//!   waits for its thread to stop (a moment), replacing any older waiting
//!   one. The lab hears each solve's progress.
//! - `shape-lab-cancel` cancels the running solve and the waiting one.
//! - `shape-lab-apply` rewrites the `:shape`, `:made-of`, `:modes`,
//!   `:resolution`, `:block` and `:points` arguments of the script's `(… solid 'id …)`
//!   call (one undoable edit) and then evaluates the script as C-c C-c does,
//!   which updates its draft.

use crate::*;
use eseqlisp::parser::{Expr, ExprKind, Parser, SpannedASTParser};

pub(super) const COMMANDS: &[&str] = &["shape-lab-load", "shape-lab-solve", "shape-lab-cancel", "shape-lab-apply"];

/// The lab's buffer (content/ui/modal/shape-lab.lisp `effect-buffer`).
const LAB_BUFFER: &str = "*shape-lab*";

/// The Lisp side's entry points.
const LOADED: &str = "eseq.modal.shape-lab/loaded";
const SOLVED: &str = "eseq.modal.shape-lab/solved";
const DRAFT_STATUS: &str = "eseq.modal.shape-lab/draft-status";
const SOLVE_PROGRESS: &str = "eseq.modal.shape-lab/solve-progress";

pub(crate) struct ShapeSolveRequest {
    ticket: f64,
    spec: sequencer::modal_fe::SolidSpec,
    base: PathBuf,
}

pub(crate) struct PendingShapeSolve {
    ticket: f64,
    progress: std::sync::Arc<sequencer::modal_fe::SolveProgress>,
    /// The progress last shown to the lab, in whole percent.
    shown: Option<(sequencer::modal_fe::SolvePhase, u32)>,
    receiver: std::sync::mpsc::Receiver<Result<(PathBuf, String), String>>,
}

/// The solve running now and the request waiting for it, and the draft an
/// Update draft is waiting to finish compiling (its key).
#[derive(Default)]
pub(crate) struct ShapeSolves {
    running: Option<PendingShapeSolve>,
    waiting: Option<ShapeSolveRequest>,
    applying: Option<String>,
}

impl ShapeSolves {
    /// Whether a solve or an Update draft is under way (the event loop keeps
    /// ticking to show its progress).
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || self.applying.is_some()
    }

    /// The tickets of the running solve and the waiting one.
    #[cfg(test)]
    pub(crate) fn tickets(&self) -> (Option<f64>, Option<f64>) {
        (self.running.as_ref().map(|r| r.ticket), self.waiting.as_ref().map(|w| w.ticket))
    }
}

fn start_solve(request: ShapeSolveRequest) -> PendingShapeSolve {
    let (sender, receiver) = std::sync::mpsc::channel();
    let ShapeSolveRequest { ticket, spec, base } = request;
    let progress = std::sync::Arc::new(sequencer::modal_fe::SolveProgress::default());
    let watched = progress.clone();
    std::thread::spawn(move || {
        let _ = sender.send(sequencer::lisp_host::solve_solid_cached(&spec, &base, &watched));
    });
    PendingShapeSolve { ticket, progress, shown: None, receiver }
}

/// Show the lab the running solve's progress (when it moved a percent),
/// hand it a finished solve, and start the waiting one. A cancelled solve
/// is not reported: the lab has moved on from it.
pub(crate) fn poll_shape_solves(editor: &mut Editor, solves: &mut ShapeSolves) {
    let Some(running) = solves.running.as_mut() else { return };
    let outcome = match running.receiver.try_recv() {
        Ok(outcome) => outcome,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            let (phase, done) = running.progress.snapshot();
            let percent = (done * 100.0) as u32;
            if running.shown != Some((phase, percent)) && !running.progress.is_cancelled() {
                running.shown = Some((phase, percent));
                let args = vec![Value::Number(running.ticket), Value::String(phase.name().into()), Value::Number(done)];
                if let Err(error) = editor.runtime_mut().invoke_global(SOLVE_PROGRESS, args) {
                    editor.show_transient_message(format!("{error:?}"));
                }
                refresh(editor);
            }
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the solver thread crashed".to_string()),
    };
    let ticket = running.ticket;
    let cancelled = running.progress.is_cancelled();
    solves.running = solves.waiting.take().map(start_solve);
    if cancelled {
        return;
    }
    let result = outcome
        .and_then(|(path, text)| solve_summary(&path, &text))
        .unwrap_or_else(|error| crate::values::map_value([("error", Value::String(error))]));
    if let Err(error) = editor.runtime_mut().invoke_global(SOLVED, vec![Value::Number(ticket), result]) {
        editor.show_transient_message(format!("{error:?}"));
    }
    refresh(editor);
}

fn refresh(editor: &mut Editor) {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

/// Cancel the running solve and drop the waiting one.
pub(crate) fn cancel(solves: &mut ShapeSolves) {
    solves.waiting = None;
    if let Some(running) = &solves.running {
        running.progress.cancel();
    }
}

/// Tell the lab how its Update draft is going: `:compiling`, `:done`, or
/// `:failed` with the error.
fn draft_status(editor: &mut Editor, status: &str, error: Option<String>) {
    let args = vec![Value::Keyword(status.to_string()), error.map_or(Value::Nil, Value::String)];
    if let Err(error) = editor.runtime_mut().invoke_global(DRAFT_STATUS, args) {
        editor.show_transient_message(format!("{error:?}"));
    }
}

/// Hand the lab the outcome of the draft compile its Update draft started
/// (`finished`: what [`super::drafts::poll_draft_compiles`] returned).
pub(crate) fn poll_draft_outcomes(editor: &mut Editor, solves: &mut ShapeSolves, finished: Vec<(String, Result<(), String>)>) {
    let Some(key) = solves.applying.clone() else { return };
    let Some((_, outcome)) = finished.into_iter().find(|(done, _)| *done == key) else { return };
    solves.applying = None;
    match outcome {
        Ok(()) => draft_status(editor, "done", None),
        Err(error) => draft_status(editor, "failed", Some(error)),
    }
    refresh(editor);
}

/// What the lab shows of a solve: `(dict :file :freqs :mass)`. The shapes
/// stay in the file, which the lab's 3-D view reads itself.
fn solve_summary(path: &Path, text: &str) -> Result<Value, String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let freqs = json["freqs"].as_array().ok_or("the solve has no freqs")?;
    let freqs = freqs.iter().filter_map(|f| f.as_f64()).map(Value::Number);
    Ok(crate::values::map_value([
        ("file", Value::String(path.display().to_string())),
        ("freqs", crate::values::list_value(freqs)),
        ("mass", Value::Number(json["mass"].as_f64().unwrap_or(0.0))),
    ]))
}

pub(super) fn handle(name: &str, payload: Value, app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let result = match name {
        "shape-lab-load" => load(editor, &payload),
        "shape-lab-solve" => solve(&mut ctx.sessions.shape_solves, &payload),
        "shape-lab-cancel" => Ok(cancel(&mut ctx.sessions.shape_solves)),
        "shape-lab-apply" => apply(app, editor, ctx, &payload),
        _ => Ok(()),
    };
    if let Err(error) = result {
        editor.show_transient_message(error);
    }
    refresh(editor);
}

/// The script buffer `payload :buffer` names: its text and path (an unsaved
/// buffer resolves relative files from the working folder, as C-c C-c does).
fn script_buffer(editor: &Editor, payload: &Value) -> Result<(String, PathBuf, String), String> {
    let name = extract_string_from_payload(payload, "buffer").ok_or("Missing script buffer")?;
    let buffer = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == name)
        .ok_or_else(|| format!("No buffer named {name}"))?;
    let text = buffer.text();
    if !super::drafts::is_script_buffer(buffer, &text) {
        return Err(format!("{name} is not a script"));
    }
    Ok((name, super::drafts::script_path(buffer), text))
}

/// Evaluate the script and send the lab `(loaded buffer dir solids wiring)`:
/// one `(dict :id :spec :mesh-file :freqs :mass :points :place :editable)` per
/// `m/solid` (`:points` its named points, a dict, or nil; `:place` where it
/// sits, or nil), `wiring` the patch's `(dict :strings :connections :listens)`,
/// `:editable` when the script has a `(… solid 'id …)` call to write into.
/// Then show the lab beside the script.
pub(crate) fn load(editor: &mut Editor, payload: &Value) -> Result<(), String> {
    let (name, path, script) = script_buffer(editor, payload)?;
    let value = sequencer::lisp_host::eval_script(editor.runtime_mut(), path.clone(), &script)
        .map_err(|errors| errors.join("; "))?
        .unwrap_or(Value::Nil);
    let calls = solid_calls(&script).unwrap_or_default();
    let objects = match field(&value, "objects") {
        Some(Value::List(objects)) => objects,
        _ => return Err(format!("{name} does not return an eseq.modal patch")),
    };
    let solids: Vec<Value> = objects
        .iter()
        .filter_map(|object| {
            let object = object.borrow();
            let spec = field(&object, "fe-spec")?;
            let id = match field(&object, "id") {
                Some(Value::Symbol(id) | Value::String(id) | Value::Keyword(id)) => id,
                _ => return None,
            };
            let data = field(&object, "data").unwrap_or(Value::Nil);
            Some(crate::values::map_value([
                ("editable", Value::Bool(calls.iter().any(|call| call.id == id))),
                ("id", Value::String(id)),
                ("spec", spec),
                ("mesh-file", field(&object, "mesh-file").unwrap_or(Value::Nil)),
                ("freqs", field(&data, "freqs").unwrap_or(Value::Nil)),
                ("mass", field(&data, "mass").unwrap_or(Value::Nil)),
                ("points", field(&object, "named-points").unwrap_or(Value::Nil)),
                ("place", field(&object, "place").unwrap_or(Value::Nil)),
            ]))
        })
        .collect();
    // what the instrument view draws besides the solids: the strings, and
    // the connections and listens that place them and sit on them
    let strings: Vec<Value> = objects
        .iter()
        .filter_map(|object| {
            let object = object.borrow();
            let is_string = matches!(field(&object, "kind"), Some(Value::Keyword(kind)) if kind.trim_start_matches(':') == "string");
            is_string.then(|| field(&object, "id")).flatten()
        })
        .collect();
    let wiring = crate::values::map_value([
        ("strings", crate::values::list_value(strings)),
        ("connections", field(&value, "connections").unwrap_or(Value::Nil)),
        ("listens", field(&value, "listens").unwrap_or(Value::Nil)),
    ]);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    editor
        .runtime_mut()
        .invoke_global(
            LOADED,
            vec![
                Value::String(name.clone()),
                Value::String(dir.display().to_string()),
                crate::values::list_value(solids),
                wiring,
            ],
        )
        .map_err(|error| format!("{error:?}"))?;
    if !editor.buffer_named_is_showing(LAB_BUFFER) && !editor.show_buffer_beside(&name, LAB_BUFFER) {
        editor.switch_active_tile_to_buffer_named(LAB_BUFFER);
    }
    Ok(())
}

/// Queue `(dict :ticket n :spec spec :dir dir)` for a background solve.
pub(crate) fn solve(solves: &mut ShapeSolves, payload: &Value) -> Result<(), String> {
    let ticket = match field(payload, "ticket") {
        Some(Value::Number(ticket)) => ticket,
        _ => return Err("Missing solve ticket".into()),
    };
    let dir = PathBuf::from(extract_string_from_payload(payload, "dir").unwrap_or_default());
    let (spec, base) = sequencer::lisp_host::solid_spec(&field(payload, "spec").unwrap_or(Value::Nil), &dir)?;
    let request = ShapeSolveRequest { ticket, spec, base };
    if let Some(running) = &solves.running {
        // the lab only wants the newest: stop the running solve, then this
        running.progress.cancel();
        solves.waiting = Some(request);
    } else {
        solves.running = Some(start_solve(request));
    }
    Ok(())
}

/// Write `(dict :buffer :id :shape :made-of :modes :resolution :block :points
/// :place)` into the script's call for solid `id` (`:made-of` a material
/// name; all but `:shape` optional; `:block :free` removes the call's block,
/// and an empty `:points` or `:place` dict its points or place), then
/// evaluate the script, updating its draft.
fn apply(app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>, payload: &Value) -> Result<(), String> {
    use super::drafts::ScriptEvaluation;
    ctx.sessions.shape_solves.applying = None;
    let name = match write_back(editor, payload) {
        Ok(name) => name,
        Err(error) => {
            draft_status(editor, "failed", Some(error));
            return Ok(());
        }
    };
    match super::drafts::evaluate_script_buffer_named(app, editor, ctx, &name) {
        ScriptEvaluation::Compiling(key) => {
            ctx.sessions.shape_solves.applying = Some(key);
            draft_status(editor, "compiling", None);
        }
        ScriptEvaluation::Created => draft_status(editor, "done", None),
        ScriptEvaluation::Failed(error) => draft_status(editor, "failed", Some(error)),
        ScriptEvaluation::Value => draft_status(editor, "failed", Some(format!("{name} does not evaluate to a program"))),
        ScriptEvaluation::NotAScript => draft_status(editor, "failed", Some(format!("{name} is not a script"))),
    }
    Ok(())
}

/// The script edit of [`apply`], as one undoable edit: the script buffer's
/// name.
pub(crate) fn write_back(editor: &mut Editor, payload: &Value) -> Result<String, String> {
    let (name, _, script) = script_buffer(editor, payload)?;
    let id = extract_string_from_payload(payload, "id").ok_or("Missing solid id")?;
    let given = |key: &str| field(payload, key).filter(|value| !matches!(value, Value::Nil));
    let edit = SolidEdit {
        shape: given("shape").ok_or("Missing shape")?,
        material: extract_string_from_payload(payload, "made-of"),
        modes: match given("modes") {
            Some(Value::Number(modes)) => Some(modes),
            _ => None,
        },
        resolution: given("resolution"),
        block: given("block").map(|block| match block {
            Value::Keyword(k) if k.trim_start_matches(':') == "free" => None,
            block => Some(block),
        }),
        points: removable(given("points")),
        place: removable(given("place")),
    };
    let text = rewrite_solid(&script, &id, &edit)?;
    if text != script && !editor.replace_buffer_text_undoable(&name, &text) {
        return Err(format!("Could not edit {name}"));
    }
    Ok(name)
}

/// A dict field to write: an empty dict removes it.
fn removable(value: Option<Value>) -> Option<Option<Value>> {
    value.map(|value| match &value {
        Value::Map(map) if map.is_empty() => None,
        _ => Some(value),
    })
}

/// What `shape-lab-apply` writes into a solid's call; `None` leaves an
/// argument as the script has it.
struct SolidEdit {
    shape: Value,
    material: Option<String>,
    modes: Option<f64>,
    resolution: Option<Value>,
    /// `Some(None)` removes the call's `:block` (the solid is free).
    block: Option<Option<Value>>,
    /// Its named points; `Some(None)` removes the call's `:points`.
    points: Option<Option<Value>>,
    /// Where it sits; `Some(None)` removes the call's `:place`.
    place: Option<Option<Value>>,
}

/// A `(… solid 'id …)` call in a script: its id, its span, and its keyword
/// arguments' value spans.
#[derive(Debug)]
struct SolidCall {
    id: String,
    /// Where an argument the call lacks goes: just before its `)`.
    insert_at: usize,
    args: Vec<(String, std::ops::Range<usize>)>,
    /// Each argument's keyword start, for removing it.
    keys_at: Vec<usize>,
}

/// Every `solid` call in `source` (bare or module-qualified, at any depth)
/// whose id is a quoted symbol.
fn solid_calls(source: &str) -> Result<Vec<SolidCall>, String> {
    let tokens = Parser::new(source.to_string()).parse_spanned().map_err(|error| format!("{error:?}"))?;
    let exprs = SpannedASTParser::new(tokens).parse().map_err(|error| format!("{error:?}"))?;
    let mut calls = Vec::new();
    fn walk(expr: &Expr, calls: &mut Vec<SolidCall>) {
        let (ExprKind::List(items) | ExprKind::QuoteList(items)) = &expr.kind else { return };
        if let [head, id, rest @ ..] = items.as_slice() {
            let is_solid = matches!(&head.kind, ExprKind::Symbol(name) if name == "solid" || name.ends_with("/solid"));
            if let (true, ExprKind::QuoteSymbol(id)) = (is_solid, &id.kind) {
                let (args, keys_at) = rest
                    .chunks(2)
                    .filter_map(|pair| match pair {
                        [key_expr @ Expr { kind: ExprKind::Keyword(key), .. }, value] => {
                            let span = &value.origin.primary_span;
                            let arg = (key.trim_start_matches(':').to_string(), span.start_byte..span.end_byte);
                            Some((arg, key_expr.origin.primary_span.start_byte))
                        }
                        _ => None,
                    })
                    .unzip();
                let span = &expr.origin.primary_span;
                calls.push(SolidCall { id: id.clone(), insert_at: span.end_byte - 1, args, keys_at });
            }
        }
        items.iter().for_each(|item| walk(item, calls));
    }
    exprs.iter().for_each(|expr| walk(expr, &mut calls));
    Ok(calls)
}

/// `source` with solid `id`'s call edited as `edit` says; an argument the
/// call lacks is added at its end, and a removed one goes with the space
/// before it. A material keeps the module alias the script wrote
/// (`mat/bronze` -> `mat/brass`).
fn rewrite_solid(source: &str, id: &str, edit: &SolidEdit) -> Result<String, String> {
    let calls = solid_calls(source)?;
    let call = match calls.iter().filter(|call| call.id == id).collect::<Vec<_>>().as_slice() {
        [call] => *call,
        [] => return Err(format!("The script has no (solid '{id} …) call")),
        _ => return Err(format!("The script has more than one (solid '{id} …) call")),
    };
    let span = |key: &str| call.args.iter().find(|(name, _)| name == key).map(|(_, span)| span.clone());
    // the column the shape starts at, for its continuation lines
    let shape_indent = span("shape").map_or(0, |span| span.start - source[..span.start].rfind('\n').map_or(0, |at| at + 1));
    // (key, its new source, or None to remove it)
    let mut edits = vec![("shape", Some(shape_source(&edit.shape, shape_indent)))];
    if let Some(material) = &edit.material {
        let alias = span("made-of")
            .and_then(|span| source[span].rsplit_once('/').map(|(alias, _)| alias.to_string()))
            .unwrap_or_else(|| "eseq.modal.materials".to_string());
        edits.push(("made-of", Some(format!("{alias}/{material}"))));
    }
    if let Some(modes) = edit.modes {
        edits.push(("modes", Some(number_source(modes))));
    }
    if let Some(resolution) = &edit.resolution {
        edits.push(("resolution", Some(shape_source(resolution, 0))));
    }
    if let Some(block) = &edit.block {
        edits.push(("block", block.as_ref().map(|block| shape_source(block, 0))));
    }
    // a call without points (or a place) gets them on a line of their own
    // under :shape
    let column_of = |at: usize| at - source[..at].rfind('\n').map_or(0, |at| at + 1);
    let shape_key_column = call.args.iter().position(|(name, _)| name == "shape").map(|k| column_of(call.keys_at[k]));
    let mut new_lines = Vec::new();
    let line_args: [(&str, &Option<Option<Value>>, fn(&Value, usize) -> String); 2] =
        [("points", &edit.points, points_source), ("place", &edit.place, shape_source)];
    for (key, value, to_source) in line_args {
        let Some(value) = value else { continue };
        match (span(key), value, shape_key_column) {
            (None, Some(value), Some(key_column)) => {
                let text = to_source(value, key_column + key.len() + 2);
                new_lines.push(format!("\n{}:{key} {text}", " ".repeat(key_column)));
            }
            (span, value, _) => {
                let column = span.map_or(shape_indent, |span| column_of(span.start));
                edits.push((key, value.as_ref().map(|value| to_source(value, column))));
            }
        }
    }
    // apply from the end of the source back, so earlier spans stay valid;
    // additions at the same place go in in order
    let mut placed: Vec<(usize, std::ops::Range<usize>, String)> = edits
        .into_iter()
        .enumerate()
        .filter_map(|(order, (key, text))| match (span(key), text) {
            (Some(span), Some(text)) => Some((order, span, text)),
            (None, Some(text)) => Some((order, call.insert_at..call.insert_at, format!(" :{key} {text}"))),
            (Some(span), None) => {
                let key_at = call.keys_at[call.args.iter().position(|(name, _)| name == key)?];
                let from = source[..key_at].trim_end().len();
                Some((order, from..span.end, String::new()))
            }
            (None, None) => None,
        })
        .collect();
    for (k, line) in new_lines.into_iter().enumerate() {
        placed.push((usize::MAX - 2 + k, call.insert_at..call.insert_at, line));
    }
    placed.sort_by_key(|(order, span, _)| std::cmp::Reverse((span.start, *order)));
    let mut text = source.to_string();
    for (_, span, replacement) in placed {
        text.replace_range(span, &replacement);
    }
    Ok(text)
}

/// A shape value as source: dicts as `(dict :k v …)` (keys sorted), lists
/// as `(list …)`, a list of lists one item per line under the first.
/// `indent` is the column the form starts at.
fn shape_source(value: &Value, indent: usize) -> String {
    match value {
        Value::Number(n) => number_source(*n),
        // eseqlisp's own literal: a Rust {:?} escape is not eseqlisp syntax
        Value::String(text) => eseqlisp::parser::string_literal(text),
        Value::Keyword(k) => format!(":{}", k.trim_start_matches(':')),
        Value::Symbol(s) => format!("'{s}"),
        Value::Bool(b) => b.to_string(),
        Value::List(items) => {
            let items: Vec<Value> = items.iter().map(|item| item.borrow().clone()).collect();
            let nested = items.iter().any(|item| matches!(item, Value::List(_) | Value::Map(_)));
            let column = indent + "(list ".len();
            let parts: Vec<String> = items.iter().map(|item| shape_source(item, column)).collect();
            let separator = if nested && parts.len() > 1 { format!("\n{}", " ".repeat(column)) } else { " ".to_string() };
            format!("(list {})", parts.join(&separator))
        }
        Value::Map(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            // each pair starts in the column after "(dict "; once a value
            // spans lines, the rest start lines of their own there
            let column = indent + "(dict ".len();
            let pairs: Vec<String> = keys
                .iter()
                .map(|key| {
                    let key_text = format!(":{} ", key.trim_start_matches(':'));
                    let value = shape_source(&map[*key].borrow(), column + key_text.len());
                    key_text + &value
                })
                .collect();
            let separator = if pairs.iter().any(|pair| pair.contains('\n')) {
                format!("\n{}", " ".repeat(column))
            } else {
                " ".to_string()
            };
            format!("(dict {})", pairs.join(&separator))
        }
        _ => "nil".to_string(),
    }
}

/// Named points as source: `(dict :name (list x y z)` one point per line,
/// in name order. `indent` is the column the form starts at.
fn points_source(points: &Value, indent: usize) -> String {
    let Value::Map(map) = points else { return shape_source(points, indent) };
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let pairs: Vec<String> = keys
        .iter()
        .map(|key| format!(":{} {}", key.trim_start_matches(':'), shape_source(&map[*key].borrow(), 0)))
        .collect();
    format!("(dict {})", pairs.join(&format!("\n{}", " ".repeat(indent + "(dict ".len()))))
}

/// A number as the shortest source that reads back as it (to 6 significant
/// figures: a dragged value carries float noise).
fn number_source(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let rounded: f64 = format!("{n:.6e}").parse().unwrap_or(n);
    format!("{rounded}")
}

fn field(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Map(map) => map.get(key).map(|item| item.borrow().clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BELL: &str = "(m/patch :name \"bell\"
  :objects (list (m/mallet 'clapper)
                 (m/solid 'bell :made-of mat/bronze :modes 40 :root 524
                          :shape (dict :revolve (dict :profile (list (list 0 0.25 0.012) (list 0.15 0 0.016))))))
  :listens (list (m/listen 'bell (list 0.09 0.05 0.2))))";

    fn edit(shape: &Value, material: Option<&str>, modes: Option<f64>) -> SolidEdit {
        SolidEdit {
            shape: shape.clone(),
            material: material.map(str::to_string),
            modes,
            resolution: None,
            block: None,
            points: None,
            place: None,
        }
    }

    fn eval(source: &str) -> Value {
        let mut vm = eseqlisp::vm::VM::new(Vec::new());
        eseqlisp::vm::register_core_natives(&mut vm);
        vm.eval_str(source).unwrap().unwrap_or(Value::Nil)
    }

    #[test]
    fn finds_the_solid_calls_and_their_arguments() {
        let calls = solid_calls(BELL).unwrap();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let keys: Vec<&str> = calls[0].args.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["made-of", "modes", "root", "shape"]);
        let made_of = calls[0].args[0].1.clone();
        assert_eq!(&BELL[made_of], "mat/bronze");
    }

    /// The new shape, material and mode count land in the call, aligned
    /// under the old shape; the rest of the script is untouched, and the
    /// result reads back as the shape it was written from.
    #[test]
    fn rewrites_a_solids_shape_material_and_modes() {
        let shape = eval("(dict :revolve (dict :profile (list (list 0 0.3 0.01) (list 0.12 0.0 0.02)) :segments (dict :along 20)))");
        let text = rewrite_solid(BELL, "bell", &edit(&shape, Some("brass"), Some(24.0))).unwrap();
        assert!(text.contains("(m/solid 'bell :made-of mat/brass :modes 24 :root 524"), "{text}");
        // the profile's second point lines up under its first
        let first = text.find("(list (list 0 0.3 0.01)").unwrap() + "(list ".len();
        let column = first - text[..first].rfind('\n').unwrap() - 1;
        assert!(text.contains(&format!("\n{}(list 0.12 0 0.02))", " ".repeat(column))), "{text}");
        assert!(text.ends_with(":listens (list (m/listen 'bell (list 0.09 0.05 0.2))))"), "{text}");
        let calls = solid_calls(&text).unwrap();
        let shape_span = calls[0].args.iter().find(|(key, _)| key == "shape").unwrap().1.clone();
        let reread = eval(&text[shape_span]);
        assert_eq!(shape_source(&reread, 0), shape_source(&shape, 0));
    }

    #[test]
    fn adds_the_arguments_a_call_lacks() {
        let script = "(m/patch :objects (list (m/solid 'bar)))";
        let shape = eval("(dict :box (dict :size (list 0.3 0.03 0.01)))");
        let text = rewrite_solid(script, "bar", &edit(&shape, Some("steel"), None)).unwrap();
        assert_eq!(
            text,
            "(m/patch :objects (list (m/solid 'bar :shape (dict :box (dict :size (list 0.3 0.03 0.01))) :made-of eseq.modal.materials/steel)))"
        );
        assert!(rewrite_solid(script, "bell", &edit(&shape, None, None)).unwrap_err().contains("no (solid 'bell"));
    }

    /// The example bell rewritten with its own shape and a new material: it
    /// still parses, its shape reads back the same, and nothing outside
    /// the call changes.
    #[test]
    fn rewrites_the_example_bell_in_place() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/modal/examples/fe-bell.lisp");
        let source = std::fs::read_to_string(path).unwrap();
        let call = solid_calls(&source).unwrap().into_iter().find(|call| call.id == "bell").unwrap();
        let span = |calls: &[SolidCall], key: &str| {
            calls.iter().find(|c| c.id == "bell").unwrap().args.iter().find(|(k, _)| k == key).unwrap().1.clone()
        };
        let old_shape = &source[span(std::slice::from_ref(&call), "shape")];
        let shape = eval(old_shape);
        let text = rewrite_solid(&source, "bell", &edit(&shape, Some("brass"), Some(40.0))).unwrap();
        if std::env::var_os("SHAPE_LAB_PRINT").is_some() {
            println!("{text}");
        }
        let calls = solid_calls(&text).unwrap();
        assert_eq!(shape_source(&eval(&text[span(&calls, "shape")]), 0), shape_source(&shape, 0));
        assert_eq!(&text[span(&calls, "made-of")], "mat/brass");
        let before = &source[..call.args[0].1.start];
        assert!(text.starts_with(before));
        assert!(text.ends_with(&source[call.insert_at..]), "{text}");
    }

    /// A drilled bar: the difference's parts one per line under its first,
    /// and the mesh detail it needs written beside it.
    #[test]
    fn writes_a_cut_shape_and_its_resolution() {
        let script = "(m/patch :objects (list (m/solid 'bar :resolution (dict :cells 32)\n                          :shape (dict :box (dict :size (list 0.3 0.04 0.012))))))";
        let shape = eval(
            "(dict :difference (list (dict :box (dict :size (list 0.3 0.04 0.012)))
                                     (dict :cylinder (dict :axis :z :center (list 0.08 0 0) :height 0.02 :radius 0.004))))",
        );
        let resolution = eval("(dict :cells 96)");
        let text = rewrite_solid(script, "bar", &SolidEdit { resolution: Some(resolution), ..edit(&shape, None, None) }).unwrap();
        assert!(text.contains("(m/solid 'bar :resolution (dict :cells 96)"), "{text}");
        let start = text.find("(dict :difference (list ").unwrap();
        let column = start - text[..start].rfind('\n').unwrap() - 1 + "(dict :difference (list ".len();
        assert!(
            text.contains(&format!("\n{}(dict :cylinder (dict :axis :z :center (list 0.08 0 0) :height 0.02 :radius 0.004))", " ".repeat(column))),
            "{text}"
        );
        let calls = solid_calls(&text).unwrap();
        let shape_span = calls[0].args.iter().find(|(key, _)| key == "shape").unwrap().1.clone();
        assert_eq!(shape_source(&eval(&text[shape_span]), 0), shape_source(&shape, 0));
    }

    /// A moved clamp replaces the call's block; a freed solid loses it,
    /// line and all.
    #[test]
    fn moves_or_removes_a_solids_block() {
        let script = "(m/patch :objects (list (m/solid 'ride :made-of mat/bronze\n                          :block (dict :sphere (dict :center (list 0 0 0.046) :radius 0.008))\n                          :shape (dict :box (dict :size (list 0.3 0.04 0.012))))))";
        let shape = eval("(dict :box (dict :size (list 0.3 0.04 0.012)))");
        let moved = eval("(dict :sphere (dict :center (list 0 0 0.002) :radius 0.008))");
        let text = rewrite_solid(script, "ride", &SolidEdit { block: Some(Some(moved)), ..edit(&shape, None, None) }).unwrap();
        assert!(text.contains(":block (dict :sphere (dict :center (list 0 0 0.002) :radius 0.008))"), "{text}");
        let text = rewrite_solid(script, "ride", &SolidEdit { block: Some(None), ..edit(&shape, None, None) }).unwrap();
        assert_eq!(
            text,
            "(m/patch :objects (list (m/solid 'ride :made-of mat/bronze\n                          :shape (dict :box (dict :size (list 0.3 0.04 0.012))))))"
        );
        // freeing a solid with no block changes nothing else
        let free = "(m/patch :objects (list (m/solid 'bar :shape (dict :box (dict :size (list 0.3 0.04 0.012))))))";
        assert_eq!(rewrite_solid(free, "bar", &SolidEdit { block: Some(None), ..edit(&shape, None, None) }).unwrap(), free);
    }

    /// Named points are written one per line under :shape, replaced in
    /// place, and removed with an empty dict.
    #[test]
    fn writes_a_solids_named_points() {
        let script = "(m/patch :objects (list (m/solid 'bar :made-of mat/steel\n                          :shape (dict :box (dict :size (list 0.3 0.04 0.012))))))";
        let shape = eval("(dict :box (dict :size (list 0.3 0.04 0.012)))");
        let points = eval("(dict :tip (list 0.15 0 0.006) :mid (list 0 0 0.006))");
        let with = |points: Option<Option<Value>>, source: &str| {
            rewrite_solid(source, "bar", &SolidEdit { points, ..edit(&shape, None, None) }).unwrap()
        };
        let added = with(Some(Some(points)), script);
        let pad = " ".repeat(26);
        assert_eq!(
            added,
            format!(
                "(m/patch :objects (list (m/solid 'bar :made-of mat/steel\n{pad}:shape (dict :box (dict :size (list 0.3 0.04 0.012)))\n{pad}:points (dict :mid (list 0 0 0.006)\n{pad}              :tip (list 0.15 0 0.006)))))"
            )
        );
        let calls = solid_calls(&added).unwrap();
        let span = calls[0].args.iter().find(|(key, _)| key == "points").unwrap().1.clone();
        assert_eq!(shape_source(&eval(&added[span]), 0), "(dict :mid (list 0 0 0.006) :tip (list 0.15 0 0.006))");
        let moved = with(Some(Some(eval("(dict :tip (list 0.1 0 0.006))"))), &added);
        assert!(moved.contains(&format!("\n{pad}:points (dict :tip (list 0.1 0 0.006)))))")), "{moved}");
        assert_eq!(with(Some(None), &added), script);
        assert_eq!(with(None, &added), added);
    }

    #[test]
    fn writes_a_solids_place_after_its_points() {
        let script = "(m/patch :objects (list (m/solid 'neck :made-of mat/steel\n                          :shape (dict :box (dict :size (list 0.4 0.05 0.02))))))";
        let shape = eval("(dict :box (dict :size (list 0.4 0.05 0.02)))");
        let place = eval("(dict :at (list 0.24 0 0.011) :turn (list 0 -3 0))");
        let points = eval("(dict :heel (list 0 0 -0.01))");
        let with = |points: Option<Option<Value>>, place: Option<Option<Value>>, source: &str| {
            rewrite_solid(source, "neck", &SolidEdit { points, place, ..edit(&shape, None, None) }).unwrap()
        };
        let added = with(Some(Some(points)), Some(Some(place)), script);
        let pad = " ".repeat(26);
        assert!(
            added.ends_with(&format!(
                "\n{pad}:points (dict :heel (list 0 0 -0.01))\n{pad}:place (dict :at (list 0.24 0 0.011) :turn (list 0 -3 0)))))"
            )),
            "{added}"
        );
        let moved = with(None, Some(Some(eval("(dict :at (list 0.3 0 0))"))), &added);
        assert!(moved.ends_with(&format!("\n{pad}:place (dict :at (list 0.3 0 0)))))")), "{moved}");
        let removed = with(None, Some(None), &moved);
        assert!(!removed.contains(":place") && removed.contains(":points"), "{removed}");
    }

    #[test]
    fn numbers_drop_float_noise() {
        assert_eq!(number_source(0.30000000000000004), "0.3");
        assert_eq!(number_source(12.0), "12");
        assert_eq!(number_source(-0.0125), "-0.0125");
    }
}
