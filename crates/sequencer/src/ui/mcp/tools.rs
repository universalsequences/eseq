//! The agent tools. Host tools need the editor, the module files or the
//! renderer and are written here; any module can add more in Lisp with
//! `(eseq.agent-tools/define-agent-tool …)`, and they are listed and called
//! through the same registry (host tools win a name clash).
//!
//! Every tool answers, failures included: an error is a result with
//! `isError`, carrying what the agent needs to fix the call.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use base64::Engine as _;
use eseqlisp::lang::def_header::first_doc_line;
use eseqlisp::vm::{VMError, Value};
use serde_json::{json, Value as Json};

use sequencer::agent::harness::clip;
use sequencer::agent::forms;
use sequencer::agent::protocol::ToolSpec;

use super::super::capture::HeadlessSession;
use super::stdio::capture_output;

/// The Lisp module that holds the tool registry.
const REGISTRY_MODULE: &str = "eseq.agent-tools";
const DEFAULT_CAPTURE_SIZE: (u32, u32) = (1600, 900);
/// Longest captured log returned with a result; the head is kept, where
/// the first error is.
const MAX_LOG_CHARS: usize = 6_000;

pub(super) enum Content {
    Text(String),
    Png(Vec<u8>),
}

pub(super) struct ToolOutput {
    content: Vec<Content>,
    is_error: bool,
}

impl ToolOutput {
    fn text(text: impl Into<String>) -> Self {
        Self { content: vec![Content::Text(text.into())], is_error: false }
    }

    fn error(text: impl Into<String>) -> Self {
        Self { content: vec![Content::Text(text.into())], is_error: true }
    }

    pub(super) fn into_json(self) -> Json {
        let content: Vec<Json> = self
            .content
            .into_iter()
            .map(|item| match item {
                Content::Text(text) => json!({ "type": "text", "text": text }),
                Content::Png(png) => json!({
                    "type": "image",
                    "mimeType": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                }),
            })
            .collect();
        json!({ "content": content, "isError": self.is_error })
    }
}

/// One parameter: name, JSON Schema type, description, required.
type Param<'a> = (&'a str, &'a str, &'a str, bool);

struct HostTool {
    name: &'static str,
    description: &'static str,
    params: &'static [Param<'static>],
}

const HOST_TOOLS: &[HostTool] = &[
    HostTool {
        name: "guide",
        description: "Read an authoring guide. Topics: `sequencer` (sequencer kinds and their \
            panels: def-kind slots, tick natives, routing, packaging), `eseqlisp` (the language: \
            syntax, reactive state, widgets, modules, gotchas), `customize` (changing how the app \
            itself looks or behaves: overrides, init.lisp, packages), `instrument`, `effect`, \
            `patcher` (the patch editor: how boxes and cables map to an instrument's or effect's \
            DGenLisp, its layout sidecar, reading a patch to the user). Read the one for your task \
            before writing code.",
        params: &[("topic", "string", "sequencer, eseqlisp, customize, instrument, effect or patcher", true)],
    },
    HostTool {
        name: "apropos",
        description: "Find defined names (functions, macros, natives, kinds' fields) containing \
            every word of the query, case-insensitive, with each one's signature and first doc \
            line. Use it before calling anything you have not seen defined.",
        params: &[
            ("query", "string", "Words to match, e.g. `seq emit` or `menu`", true),
            ("limit", "integer", "Most results to return (default 40)", false),
        ],
    },
    HostTool {
        name: "describe",
        description: "Signature and documentation of one function, macro or native, by bare \
            name (`seq-emit`) or qualified name (`eseq.view-kit/menu-of`). For a kind \
            (`describe-kind`'s argument), its fields.",
        params: &[("name", "string", "The name to describe", true)],
    },
    HostTool {
        name: "list_modules",
        description: "Module names on the load path containing `filter`: your Local modules, \
            installed packages (`author.name.*`) and the factory (`eseq.*`).",
        params: &[("filter", "string", "Substring to match (default: all)", false)],
    },
    HostTool {
        name: "search_source",
        description: "Search the source of every loaded module (your Local modules, installed \
            packages, the factory), like grep: each matching line with its module, line number \
            and the definition it sits in. Use it to find where something is drawn, set or \
            called, then `form_source` that definition. Case-insensitive.",
        params: &[
            ("query", "string", "Text to find, or a regex when `regex` is true", true),
            ("filter", "string", "Only modules whose name contains this, e.g. `mixer`", false),
            ("regex", "boolean", "Treat `query` as a regular expression (default false)", false),
            ("limit", "integer", "Most lines to return (default 40, at most 200)", false),
        ],
    },
    HostTool {
        name: "outline",
        description: "A module's file and its top-level forms, one per line: line number, head, \
            name and first doc line. Read this, then `form_source` for the forms you need, \
            instead of whole files.",
        params: &[("module", "string", "Module name, e.g. alez.jaki.kind", true)],
    },
    HostTool {
        name: "form_source",
        description: "The exact source of the top-level form(s) in a module that define `name`.",
        params: &[
            ("module", "string", "Module name", true),
            ("name", "string", "The defined name (for def-sequencer, its string name)", true),
        ],
    },
    HostTool {
        name: "create_module",
        description: "Create a Local module, your writable workspace: writes `a/b.lisp` for \
            module `a.b` under the Local packages folder, starting with `(module a.b)` and \
            `source`, then imports it. Use a two-or-more-segment name you own (not eseq.*). \
            Fails, and removes the file, when the module does not load.",
        params: &[
            ("module", "string", "New module name, e.g. euclid.core (package euclid)", true),
            ("source", "string", "Forms after the (module …) line: imports, exports, definitions", false),
        ],
    },
    HostTool {
        name: "define_form",
        description: "Write one top-level form into a module's file and reload the module. A \
            definition (def, defstate, def-kind, defwidget, override, …) replaces the form \
            that defines the same name, or is appended when none does; any other form (bind-key, \
            set!) is appended. Only Local modules, installed packages and `user.init` (the user's \
            init.lisp, loaded in every project) are writable. When the \
            reload fails the file is restored and the errors are returned. This is how work \
            persists: the file is the truth.",
        params: &[
            ("module", "string", "Module to write", true),
            ("source", "string", "Exactly one top-level form", true),
        ],
    },
    HostTool {
        name: "write_module",
        description: "Replace a whole module file with `source` (which must begin with its \
            `(module NAME)` form) and reload it, for a rewrite too large for `define_form`. \
            Same rules: only Local modules and installed packages; a failed reload restores the \
            old file and returns the errors.",
        params: &[
            ("module", "string", "Module to rewrite", true),
            ("source", "string", "The complete new file", true),
        ],
    },
    HostTool {
        name: "eval",
        description: "Evaluate eseqlisp forms in eseq's UI runtime and return the last value \
            printed, with any errors logged while they ran. Host commands they queue (creating \
            instances, rack and song edits) are applied. For questions and experiments only: \
            nothing defined here is saved, so persist with `define_form`.",
        params: &[("code", "string", "One or more forms", true)],
    },
    HostTool {
        name: "new_instance",
        description: "Create an instance of a kind (a sequencer, a panel) as the Packages tab's \
            New does, and return the name of the buffer that shows it, for `capture`.",
        params: &[
            ("module", "string", "The module that declares the kind (def-kind …)", true),
            ("kind", "string", "The kind's name", true),
        ],
    },
    HostTool {
        name: "simulate",
        description: "Run a sequencer instance's tick headlessly for some bars, as Play from the \
            top would, and list every hit it emits (bar.sixteenth, track, transpose, velocity, \
            duration, chords, p-locks), or the tick that threw and why. Uses the instance's \
            document in the selected scene; touches nothing live. Use it after changing a tick, the way \
            `capture` checks a panel.",
        params: &[
            ("instance", "integer", "Instance id (default: the newest instance)", false),
            ("bars", "integer", "Bars of 4/4 to run (default 2, at most 64)", false),
        ],
    },
    HostTool {
        name: "capture",
        description: "Render one buffer to an image, as the app draws it, and report its layout: \
            every keyed widget's pixel rectangle and problems (clipped, offscreen, zero-size), \
            overlapping siblings in stacks and grids, and the hooks that restyle what is on \
            screen (its parts, the functions that build each region, the theme slots it paints \
            with). `what_draws` names the hooks at one pixel. `key` crops to one keyed subtree. To \
            operate the panel first, `click` clicks the center of a keyed widget, then `keys` \
            types keystrokes (`UP*3 RET`); check the effect with `eval` or `simulate`. Without \
            `buffer`, lists the buffers.",
        params: &[
            ("buffer", "string", "Buffer name, e.g. `*pulse · pulse 1*`", false),
            ("key", "string", "Crop to the widget with this :key", false),
            ("click", "string", "Click the center of the widget with this :key first", false),
            ("width", "integer", "Pixels (default 1600)", false),
            ("height", "integer", "Pixels (default 900)", false),
            ("keys", "string", "Keystrokes to send first, bindings spelling", false),
        ],
    },
    HostTool {
        name: "what_draws",
        description: "What draws the pixel (x, y) of a buffer, in an uncropped `capture` of the \
            same width and height, as the buffer stands now (no click or keys): the widgets under \
            it, topmost first, each with the hooks that restyle it, least fragile first: the theme \
            slots it repaints through (override-theme-slot), its part (restyle), the function \
            that built it (override :around). Open menus and modals are not looked into. Use it \
            on the spot the user means before searching source.",
        params: &[
            ("buffer", "string", "Buffer name, as for `capture`", true),
            ("x", "integer", "Pixel column in the capture", true),
            ("y", "integer", "Pixel row in the capture", true),
            ("width", "integer", "The capture's width (default 1600)", false),
            ("height", "integer", "The capture's height (default 900)", false),
        ],
    },
    HostTool {
        name: "dgen_docs",
        description: "Look up DGenLisp, the language instruments and effects are written in \
            (dsp.lisp): operators (`gswitch`, `iota`, `phasor`, `history`), special forms, \
            @attributes and example patches using them, from the compiler's own API catalog. \
            Use it before explaining or writing DSP code: DGenLisp is not eseqlisp, so \
            `apropos` and `describe` do not know it.",
        params: &[
            ("query", "string", "Operator names or words, comma-separated, e.g. `gswitch, iota` or `delay`", true),
            ("limit", "integer", "Results per kind for each query (default 4)", false),
        ],
    },
    HostTool {
        name: "dgen_values",
        description: "Work out what DGenLisp defs in a dsp.lisp come to at given param values \
            (each param missing from `knobs` takes its @default): arithmetic, comparisons, \
            gswitch, iota and math, vectors element-wise; a def reading a signal (in, \
            histories, oscillators, macros) is reported as varying per sample. For the knobs the \
            user has now, use `dgen_values_live`.",
        params: &[
            ("path", "string", "The dsp.lisp file", true),
            ("names", "string", "Defs to work out, comma-separated, e.g. `B, freqs`", true),
            ("knobs", "object", "Param values by name, e.g. {\"material\": 2}", false),
        ],
    },
    HostTool {
        name: "view_state",
        description: "The state a buffer's drawn view reads, as it was last drawn here: every \
            reactive field its widgets read (defstates, kind fields, host values like SEQ.*) \
            with its value now. Fields differing only by a number fold into one row. `capture` \
            the buffer first if it has not been drawn. For the user's own window use \
            `view_state_live`.",
        params: &[("buffer", "string", "Buffer name, as for `capture`", true)],
    },
];

/// Import the Lisp registry (and so every tool module it loads). A failure is
/// logged; the host tools still work.
pub(super) fn load_lisp_tools(session: &mut HeadlessSession) {
    recover_interrupted_writes(session);
    let (result, log) = run_lisp(session, &format!("(import {REGISTRY_MODULE})"));
    if let Err(problem) = result {
        eprintln!("eseq mcp: the Lisp tool registry did not load: {problem} {log}");
    }
    // Boot's own failures are the app's, not the first call's.
    for error in session.take_reactive_errors() {
        eprintln!("eseq mcp: at boot: {error}");
    }
}

pub(super) fn list(session: &mut HeadlessSession) -> Vec<Json> {
    let host = HOST_TOOLS.iter().map(|tool| ToolSpec {
        name: tool.name.to_string(),
        description: squash(tool.description),
        input_schema: input_schema(tool.params.iter().copied()),
    });
    let lisp: Vec<ToolSpec> = lisp_tool_specs(session)
        .into_iter()
        .filter(|spec| !HOST_TOOLS.iter().any(|tool| tool.name == spec.name))
        .collect();
    host.chain(lisp)
        .map(|spec| json!({ "name": spec.name, "description": spec.description, "inputSchema": spec.input_schema }))
        .collect()
}

pub(super) fn call(session: &mut HeadlessSession, name: &str, args: &Json) -> ToolOutput {
    let str_arg = |key: &str| args.get(key).and_then(Json::as_str).map(str::to_string);
    let need = |key: &str| str_arg(key).ok_or_else(|| format!("`{key}` is required"));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<ToolOutput, String> {
        Ok(match name {
            "guide" => guide(&need("topic")?),
            "apropos" => {
                let limit = args.get("limit").and_then(Json::as_u64).unwrap_or(40) as usize;
                apropos(session, &need("query")?, limit)
            }
            "describe" => describe(session, &need("name")?),
            "list_modules" => list_modules(session, str_arg("filter").as_deref().unwrap_or("")),
            "search_source" => {
                let limit = args.get("limit").and_then(Json::as_u64).unwrap_or(40).clamp(1, 200) as usize;
                let regex = args.get("regex").and_then(Json::as_bool).unwrap_or(false);
                let filter = str_arg("filter").unwrap_or_default();
                search_source(session, &need("query")?, &filter, regex, limit)?
            }
            "outline" => outline(session, &need("module")?)?,
            "form_source" => form_source(session, &need("module")?, &need("name")?)?,
            "create_module" => create_module(session, &need("module")?, str_arg("source").as_deref())?,
            "define_form" => define_form(session, &need("module")?, &need("source")?)?,
            "write_module" => write_module(session, &need("module")?, &need("source")?)?,
            "eval" => match run_lisp(session, &need("code")?) {
                (Ok(value), log) => ToolOutput::text(with_log(format!("=> {value}"), &log)),
                (Err(problem), log) => ToolOutput::error(with_log(problem, &log)),
            },
            "new_instance" => new_instance(session, &need("module")?, &need("kind")?)?,
            "simulate" => {
                let bars = args
                    .get("bars")
                    .and_then(Json::as_u64)
                    .map(|bars| bars.min(u64::from(sequencer::agent::simulate::MAX_BARS)) as u32)
                    .unwrap_or(2);
                let instance = args.get("instance").and_then(Json::as_u64);
                let (report, log) = capture_output(|| session.simulate(instance, bars));
                match report {
                    Ok(report) => ToolOutput::text(with_log(report, &log)),
                    Err(problem) => ToolOutput::error(with_log(problem, &log)),
                }
            }
            "capture" => {
                match str_arg("buffer") {
                    None => ToolOutput::text(format!("Buffers:\n{}", session.buffer_names().join("\n"))),
                    Some(buffer) => capture(
                        session,
                        &buffer,
                        str_arg("key").as_deref(),
                        capture_size(args),
                        str_arg("click").as_deref(),
                        str_arg("keys").as_deref(),
                    ),
                }
            }
            "what_draws" => {
                let number = |key: &str| args.get(key).and_then(Json::as_u64);
                let (Some(x), Some(y)) = (number("x"), number("y")) else {
                    return Err("`x` and `y` are required".to_string());
                };
                what_draws(session, &need("buffer")?, capture_size(args), (x as f32, y as f32))
            }
            "dgen_docs" => {
                let limit = args.get("limit").and_then(Json::as_u64).unwrap_or(4).clamp(1, 20) as usize;
                dgen_docs(&need("query")?, limit)?
            }
            "dgen_values" => {
                let path = need("path")?;
                let source = std::fs::read_to_string(&path).map_err(|why| format!("cannot read {path}: {why}"))?;
                let names: Vec<String> = need("names")?.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect();
                let knobs: std::collections::HashMap<String, f64> = args
                    .get("knobs")
                    .and_then(Json::as_object)
                    .map(|knobs| knobs.iter().filter_map(|(name, value)| Some((name.clone(), value.as_f64()?))).collect())
                    .unwrap_or_default();
                let origin = if knobs.is_empty() { "every param at its default".to_string() } else { "the knobs given".to_string() };
                ToolOutput::text(crate::agent_pane::dgen_values(&source, &knobs, &origin, &names)?)
            }
            "view_state" => ToolOutput::text(crate::agent_pane::view_state(&session.editor, &need("buffer")?)?),
            _ => call_lisp_tool(session, name, args),
        })
    }));
    let mut output = match outcome {
        Ok(Ok(output)) => output,
        Ok(Err(message)) => ToolOutput::error(message),
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            ToolOutput::error(format!("the tool panicked inside eseq: {message}"))
        }
    };
    // The first call after a restart says what an interrupted call left
    // behind and how it was put back.
    let recovered = RECOVERED.with(|notes| std::mem::take(&mut *notes.borrow_mut()));
    if !recovered.is_empty() {
        output.content.push(Content::Text(recovered.join("\n")));
    }
    // A view that throws leaves its last good tree (or none) and says
    // nothing; this is where the agent learns why a buffer did not draw.
    let errors = session.take_reactive_errors();
    if !errors.is_empty() {
        output.content.push(Content::Text(format!(
            "Render errors (a view or effect threw while this call ran):\n{}",
            errors.join("\n")
        )));
    }
    output
}

fn guide(topic: &str) -> ToolOutput {
    let paths = sequencer::app_paths::app_paths();
    let candidates: Vec<PathBuf> = match topic {
        "sequencer" | "customize" | "instrument" | "effect" | "patcher" => {
            vec![paths.authoring_dir().join(format!("{topic}-reference.md"))]
        }
        // The bundle carries the language guide beside the authoring
        // references (dist/macos/build.sh); a checkout reads it from docs/.
        "eseqlisp" => vec![
            paths.authoring_dir().join("eseqlisp-guide.md"),
            paths.factory_root().join("../docs/eseqlisp/GUIDE.md"),
        ],
        _ => return ToolOutput::error(format!("no guide `{topic}`; topics: sequencer, eseqlisp, customize, instrument, effect, patcher")),
    };
    match candidates.iter().find_map(|path| std::fs::read_to_string(path).ok()) {
        Some(text) => ToolOutput::text(text),
        None => ToolOutput::error(format!("the `{topic}` guide is not installed (looked in {candidates:?})")),
    }
}

fn apropos(session: &mut HeadlessSession, query: &str, limit: usize) -> ToolOutput {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return ToolOutput::error("`query` needs at least one word");
    }
    let runtime = session.editor.runtime_mut();
    let matches: Vec<String> = runtime
        .completion_symbols()
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            words.iter().all(|word| lower.contains(word.as_str()))
        })
        .cloned()
        .collect();
    if matches.is_empty() {
        return ToolOutput::text(format!("No names contain {query:?}."));
    }
    // A Lisp definition's own header wins over native metadata, as the
    // command palette reads it.
    let mut lines: Vec<String> = matches
        .iter()
        .take(limit)
        .map(|name| {
            let (signature, doc) = match runtime.definition_header(name) {
                Some(header) => (header.signature.clone(), header.doc.clone()),
                None => match runtime.completion_metadata_for(name) {
                    Some(meta) => (meta.signature, Some(meta.docs)),
                    None => (String::new(), None),
                },
            };
            let doc = doc.as_deref().and_then(first_doc_line).unwrap_or("(undocumented)").to_string();
            format!("{}  —  {doc}", if signature.is_empty() { name.as_str() } else { signature.as_str() })
        })
        .collect();
    if matches.len() > limit {
        lines.push(format!("… {} more; narrow the query or raise `limit`", matches.len() - limit));
    }
    ToolOutput::text(lines.join("\n"))
}

fn describe(session: &mut HeadlessSession, name: &str) -> ToolOutput {
    let mut text = session.editor.describe_function_text(name);
    let (kind, _) = capture_output(|| {
        session.editor.runtime_mut().invoke_global("describe-kind", vec![Value::String(name.to_string())])
    });
    if let Ok(Some(Value::String(fields))) = kind {
        if !fields.trim().is_empty() {
            text.push_str(&format!("\nKind `{name}`:\n{fields}\n"));
        }
    }
    ToolOutput::text(text)
}

/// Module names on the load path containing `filter`.
fn modules_matching(session: &mut HeadlessSession, filter: &str) -> Vec<String> {
    session.editor.runtime_mut().module_completions().into_iter().filter(|module| module.contains(filter)).collect()
}

fn list_modules(session: &mut HeadlessSession, filter: &str) -> ToolOutput {
    let modules = modules_matching(session, filter);
    ToolOutput::text(if modules.is_empty() {
        format!("No modules contain {filter:?}.")
    } else {
        modules.join("\n")
    })
}

fn valid_module(module: &str) -> Result<(), String> {
    if eseqlisp::modules::is_valid_module_name(module) {
        Ok(())
    } else {
        Err(format!("`{module}` is not a module name (dotted, like `euclid.core`)"))
    }
}

/// A module's file and text, resolved exactly as the loader resolves it.
fn module_source(session: &HeadlessSession, module: &str) -> Result<(PathBuf, String), String> {
    valid_module(module)?;
    session
        .editor
        .runtime()
        .module_source_for_navigation(module, None, Vec::new())
        .map(|source| (source.path, source.text))
        .map_err(|_| format!("no file for module `{module}`; `list_modules` shows what exists"))
}

/// Longest matched line shown; the definition is a `form_source` away.
const SEARCH_LINE_CHARS: usize = 160;

fn search_source(
    session: &mut HeadlessSession,
    query: &str,
    filter: &str,
    is_regex: bool,
    limit: usize,
) -> Result<ToolOutput, String> {
    if query.trim().is_empty() {
        return Err("`query` is empty".to_string());
    }
    let pattern = regex::RegexBuilder::new(&if is_regex { query.to_string() } else { regex::escape(query) })
        .case_insensitive(true)
        .build()
        .map_err(|error| format!("`query` is not a valid regex: {error}"))?;
    let modules = modules_matching(session, filter);
    let mut files = std::collections::HashSet::new();
    let (mut shown, mut total, mut matched_modules) = (Vec::new(), 0, 0);
    for module in modules {
        let Ok((path, source)) = module_source(session, &module) else { continue };
        if !files.insert(path) {
            continue;
        }
        let found = forms::search(&source, &pattern, limit.saturating_sub(shown.len()));
        matched_modules += usize::from(found.count > 0);
        total += found.count;
        for hit in found.hits {
            let form = match hit.form {
                Some((head, Some(name))) => format!("  [{head} {name}]"),
                Some((head, None)) => format!("  [{head}]"),
                None if !found.parsed => "  [unparsed]".to_string(),
                None => String::new(),
            };
            shown.push(format!("{module}:{}{form}  {}", hit.line, clip(&hit.text, SEARCH_LINE_CHARS)));
        }
    }
    if total == 0 {
        return Ok(ToolOutput::text(format!("No line matches {query:?}.")));
    }
    let more = if total > shown.len() { format!(" (showing {}; narrow with `filter` or raise `limit`)", shown.len()) } else { String::new() };
    Ok(ToolOutput::text(format!("{total} line(s) in {matched_modules} module(s){more}:\n{}", shown.join("\n"))))
}

fn outline(session: &HeadlessSession, module: &str) -> Result<ToolOutput, String> {
    let (path, source) = module_source(session, module)?;
    let forms = forms::top_level_forms(&source).map_err(|error| format!("{}:{error}", path.display()))?;
    let mut lines = vec![format!("{} ({} lines)", path.display(), source.lines().count())];
    for form in &forms {
        let text = &source[form.start_byte..form.end_byte];
        // Header forms are short and say what the module offers: show them whole.
        let summary = if matches!(form.head.as_str(), "module" | "import" | "export") {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            let name = form.name.as_deref().map(|name| format!(" {name}")).unwrap_or_default();
            let doc = form.doc.as_deref().map(|doc| format!("  ; {doc}")).unwrap_or_default();
            format!("({}{name} …){doc}", form.head)
        };
        let span = if form.end_line > form.line { format!("{}-{}", form.line, form.end_line) } else { form.line.to_string() };
        lines.push(format!("{span:>9}  {summary}"));
    }
    Ok(ToolOutput::text(lines.join("\n")))
}

fn form_source(session: &HeadlessSession, module: &str, name: &str) -> Result<ToolOutput, String> {
    let (path, source) = module_source(session, module)?;
    let found = forms::form_source(&source, name).map_err(|error| format!("{}:{error}", path.display()))?;
    if found.is_empty() {
        return Err(format!("no top-level form in {module} defines `{name}`; see `outline`"));
    }
    Ok(ToolOutput::text(
        found.into_iter().map(|(line, text)| format!(";; {}:{line}\n{text}", path.display())).collect::<Vec<_>>().join("\n\n"),
    ))
}

fn create_module(session: &mut HeadlessSession, module: &str, source: Option<&str>) -> Result<ToolOutput, String> {
    valid_module(module)?;
    if !module.contains('.') {
        return Err(format!("`{module}` needs two or more segments, like `euclid.core`"));
    }
    if module.starts_with("eseq.") {
        return Err("eseq.* names are reserved for the factory".to_string());
    }
    if let Ok((existing, _)) = module_source(session, module) {
        return Err(format!("`{module}` already exists at {}; edit it with `define_form`", existing.display()));
    }
    let local = sequencer::app_paths::app_paths().local_modules_dir();
    let path = local.join(format!("{}.lisp", module.replace('.', "/")));
    let mut text = format!("(module {module})\n");
    if let Some(source) = source.filter(|source| !source.trim().is_empty()) {
        forms::top_level_forms(source).map_err(|error| format!("source, line {error}"))?;
        text.push('\n');
        text.push_str(source.trim_end());
        text.push('\n');
    }
    std::fs::create_dir_all(path.parent().unwrap_or(&local)).map_err(|error| error.to_string())?;
    // A kill (Stop) before the import finishes skips the removal below; the
    // journal lets the next server remove the file instead.
    let _journal = WriteJournal::begin(&journal_dir(), &path, None)?;
    std::fs::write(&path, &text).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let (loaded, log) = run_lisp(session, &format!("(import {module})"));
    if let Err(problem) = loaded {
        let _ = std::fs::remove_file(&path);
        return Err(with_log(format!("`{module}` did not load, so the file was removed: {problem}"), &log));
    }
    Ok(ToolOutput::text(with_log(format!("Created {} and imported {module}.", path.display()), &log)))
}

/// Puts a file's previous text back unless disarmed: a reload that panics
/// must not leave the new text on disk.
struct RestoreFile<'a> {
    path: &'a Path,
    before: &'a str,
    armed: bool,
}

impl RestoreFile<'_> {
    fn restore(mut self) -> Result<(), String> {
        self.armed = false;
        std::fs::write(self.path, self.before)
            .map_err(|error| format!("could not restore {}: {error}", self.path.display()))
    }
}

impl Drop for RestoreFile<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::write(self.path, self.before);
        }
    }
}

thread_local! {
    /// What startup recovery put back, for the first call's result.
    static RECOVERED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Where write tools journal a file's previous text for the length of a
/// call.
fn journal_dir() -> PathBuf {
    sequencer::app_paths::app_paths().user_data_root().join("mcp-restore")
}

/// A record, on disk for the length of a write call, of what a module file
/// held before it (`before: None` when the call creates it). Dropping it
/// removes the record; a server killed mid-call (Stop sends SIGKILL, so no
/// Drop runs) leaves it for [`recover_journals`] in the next server.
struct WriteJournal {
    record: PathBuf,
}

impl WriteJournal {
    fn begin(dir: &Path, path: &Path, before: Option<&str>) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let cannot = |error: std::io::Error| format!("cannot journal the write to {}: {error}", path.display());
        std::fs::create_dir_all(dir).map_err(cannot)?;
        let pid = std::process::id();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let record = dir.join(format!("{pid}-{n}.json"));
        let partial = dir.join(format!("{pid}-{n}.partial"));
        let body = json!({ "pid": pid, "path": path, "before": before });
        // Written whole and renamed into place, so a kill never leaves half
        // a record; synced before the module file is touched.
        let write = || -> std::io::Result<()> {
            use std::io::Write as _;
            let mut file = std::fs::File::create(&partial)?;
            file.write_all(body.to_string().as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&partial, &record)
        };
        write().map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            cannot(error)
        })?;
        Ok(Self { record })
    }
}

impl Drop for WriteJournal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.record);
    }
}

/// Whether the server that wrote a journal may still be mid-call. Another
/// live server's record is its own business.
fn process_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Puts back every file a killed server's write call left changed, and
/// returns each restored path (`true` when it was rewritten, `false` when
/// a file the call created was removed) with a line saying so.
fn recover_journals(dir: &Path) -> Vec<(PathBuf, bool, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut records: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    records.sort();
    let mut recovered = Vec::new();
    for record in records {
        let extension = record.extension().and_then(|extension| extension.to_str());
        if extension == Some("partial") {
            let stem_pid = record.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.split('-').next());
            if stem_pid.and_then(|pid| pid.parse().ok()).is_some_and(|pid| !process_alive(pid)) {
                let _ = std::fs::remove_file(&record);
            }
            continue;
        }
        if extension != Some("json") {
            continue;
        }
        let Some(body) = std::fs::read_to_string(&record).ok().and_then(|text| serde_json::from_str::<Json>(&text).ok())
        else {
            eprintln!("eseq mcp: unreadable write journal {}", record.display());
            continue;
        };
        let pid = body.get("pid").and_then(Json::as_u64).and_then(|pid| u32::try_from(pid).ok());
        if pid.is_some_and(process_alive) {
            continue;
        }
        let Some(path) = body.get("path").and_then(Json::as_str).map(PathBuf::from) else {
            let _ = std::fs::remove_file(&record);
            continue;
        };
        let outcome = match body.get("before").and_then(Json::as_str) {
            Some(before) => std::fs::write(&path, before).map(|()| {
                (true, format!("An interrupted call had left {} changed; its previous text was restored.", path.display()))
            }),
            None => match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok((
                    false,
                    format!("An interrupted call had created {} without loading it; the file was removed.", path.display()),
                )),
            },
        };
        match outcome {
            Ok((rewritten, note)) => {
                let _ = std::fs::remove_file(&record);
                recovered.push((path, rewritten, note));
            }
            // Kept for the next server to retry.
            Err(error) => eprintln!("eseq mcp: could not restore {}: {error}", path.display()),
        }
    }
    recovered
}

/// Startup: undo what a call killed mid-write left on disk, reload the
/// restored files (boot may have loaded the unvalidated text) and keep the
/// notes for the first call's result.
fn recover_interrupted_writes(session: &mut HeadlessSession) {
    let recovered = recover_journals(&journal_dir());
    if recovered.is_empty() {
        return;
    }
    let rewritten: Vec<PathBuf> =
        recovered.iter().filter(|(_, rewritten, _)| *rewritten).map(|(path, _, _)| path.clone()).collect();
    let mut notes: Vec<String> = recovered.into_iter().map(|(_, _, note)| note).collect();
    if !rewritten.is_empty() {
        let (report, _log) =
            capture_output(|| session.editor.runtime_mut().reload_paths_transactional(rewritten, Vec::new()));
        if !report.success {
            notes.push(format!("Reloading the restored text failed:\n{}", report.diagnostics.join("\n")));
        }
    }
    for note in &notes {
        eprintln!("eseq mcp: {note}");
    }
    RECOVERED.with(|pending| pending.borrow_mut().extend(notes));
}

/// A user-tier module's file and text, or why the agent may not write it:
/// Local modules, installed packages and the user's init.lisp.
fn writable_module(session: &HeadlessSession, module: &str) -> Result<(PathBuf, String), String> {
    let (path, before) = module_source(session, module)?;
    let paths = sequencer::app_paths::app_paths();
    if !forms::is_under_any(&path, &forms::user_module_roots(paths))
        && !crate::lisp_hot_reload::same_path(&crate::host_commands::packages::user_init_path(), &path)
    {
        return Err(format!(
            "{} is not writable: only Local modules, installed packages and user.init (init.lisp) are. Make your own module with `create_module`",
            path.display()
        ));
    }
    Ok((path, before))
}

fn define_form(session: &mut HeadlessSession, module: &str, form: &str) -> Result<ToolOutput, String> {
    let (path, before) = writable_module(session, module)?;
    let (after, defined) = forms::define_form(&before, form)?;
    let place = match defined {
        forms::Defined::Replaced { line } => format!("Replaced the form at line {line} of"),
        forms::Defined::Appended { line } => format!("Appended at line {line} of"),
    };
    rewrite_and_reload(session, module, &path, &before, &after, &place)
}

fn write_module(session: &mut HeadlessSession, module: &str, source: &str) -> Result<ToolOutput, String> {
    let (path, before) = writable_module(session, module)?;
    forms::top_level_forms(source).map_err(|error| format!("source, line {error}"))?;
    // The first form names it, and the loader (which takes the last plain
    // `(module NAME)`) must agree.
    let loaded_as = eseqlisp::modules::inspect_exports(source).ok().and_then(|(name, _)| name);
    if forms::declared_module(source).as_deref() != Some(module) || loaded_as.as_deref() != Some(module) {
        return Err(format!("the source must begin with (module {module})"));
    }
    let mut after = source.trim_end().to_string();
    after.push('\n');
    rewrite_and_reload(session, module, &path, &before, &after, "Rewrote")
}

/// Write `after` over `before`, reload the module from disk and settle; a
/// failed reload (or a panic in it) puts `before` back.
fn rewrite_and_reload(
    session: &mut HeadlessSession,
    module: &str,
    path: &Path,
    before: &str,
    after: &str,
    what: &str,
) -> Result<ToolOutput, String> {
    // Declared first so it is dropped last, after the guard has restored.
    // Drop does not run when Stop kills the server mid-call; the journal
    // lets the next server put `before` back.
    let _journal = WriteJournal::begin(&journal_dir(), path, Some(before))?;
    let mut guard = RestoreFile { path, before, armed: true };
    std::fs::write(path, after).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    // A failed transactional reload restores the runtime's state itself.
    let (report, log) = capture_output(|| {
        session.editor.runtime_mut().reload_paths_transactional(vec![path.to_path_buf()], Vec::new())
    });
    if !report.success {
        guard.restore()?;
        return Err(with_log(
            format!("The reload failed, so {} was restored:\n{}", path.display(), report.diagnostics.join("\n")),
            &log,
        ));
    }
    guard.armed = false;
    let (settled, settle_log) = capture_output(|| session.settle());
    // The reload reports each name under its module and its aliases; list
    // this module's own once (a file with no module form has only flat ones).
    let prefix = format!("{module}/");
    let mut changed: Vec<&str> =
        report.changed_symbols.iter().filter_map(|name| name.strip_prefix(prefix.as_str())).collect();
    if changed.is_empty() {
        changed = report.changed_symbols.iter().map(String::as_str).filter(|name| !name.contains('/')).collect();
    }
    changed.sort_unstable();
    changed.dedup();
    let changed = if changed.is_empty() { String::new() } else { format!("; changed: {}", changed.join(", ")) };
    // A reload rebinds what the file defines but does not unbind what it no
    // longer does.
    let defined = |text: &str| -> Vec<String> {
        forms::top_level_forms(text).map(|forms| forms.into_iter().filter_map(|form| form.name).collect()).unwrap_or_default()
    };
    let now = defined(after);
    let removed: Vec<String> = defined(before).into_iter().filter(|name| !now.contains(name)).collect();
    let removed = if removed.is_empty() {
        String::new()
    } else {
        format!(
            "\nRemoved from the file: {}. They stay bound in this session until eseq restarts, so check \
             nothing still calls them.",
            removed.join(", ")
        )
    };
    let text = format!("{what} {} and reloaded {module}{changed}.{removed}", path.display());
    let log = format!("{log}{settle_log}");
    Ok(match settled {
        Ok(()) => ToolOutput::text(with_log(text, &log)),
        Err(problem) => ToolOutput::error(with_log(format!("{text}\nApplying its host commands failed: {problem}"), &log)),
    })
}

fn new_instance(session: &mut HeadlessSession, module: &str, kind: &str) -> Result<ToolOutput, String> {
    valid_module(module)?;
    let package = sequencer::lisp_host::package_name_for_module(module);
    let id = sequencer::lisp_host::kind_id(package.as_deref(), Some(module), kind);
    let before = session.buffer_names();
    let (imported, log) = run_lisp(session, &format!("(import {module})"));
    if let Err(problem) = imported {
        return Err(with_log(problem, &log));
    }
    let payload = Value::Map(HashMap::from([("kind".to_string(), Rc::new(RefCell::new(Value::String(id.clone()))))]));
    let (created, create_log) = capture_output(|| {
        let result = session
            .editor
            .runtime_mut()
            .invoke_global("host-command", vec![Value::String("instance-create".to_string()), payload])
            .map_err(|error| format!("{error:?}"));
        let settled = session.settle();
        result.and(settled)
    });
    let log = format!("{log}{create_log}");
    created.map_err(|problem| with_log(problem, &log))?;
    let buffers: Vec<String> = session.buffer_names().into_iter().filter(|name| !before.contains(name)).collect();
    if buffers.is_empty() {
        return Err(with_log(
            format!("no buffer appeared for kind `{id}`: the kind may have no :view, or creating it failed"),
            &log,
        ));
    }
    let instance = session.latest_instance_id().unwrap_or_default();
    Ok(ToolOutput::text(with_log(
        format!("Created `{id}` instance {instance}; its buffer: {}", buffers.join(", ")),
        &log,
    )))
}

fn capture(
    session: &mut HeadlessSession,
    buffer: &str,
    key: Option<&str>,
    size: (u32, u32),
    click: Option<&str>,
    keys: Option<&str>,
) -> ToolOutput {
    let (rendered, log) = capture_output(|| session.render_png(buffer, key, size, click, keys));
    let (png, report) = match rendered {
        Ok(rendered) => rendered,
        Err(problem) => return ToolOutput::error(with_log(problem, &log)),
    };
    let mut lines = vec![format!("{buffer} at {}x{} px; keyed widgets (x,y wxh):", size.0, size.1)];
    for node in &report.nodes {
        let place = match node.region {
            Some(r) => format!("{},{} {}x{}", r.x, r.y, r.width, r.height),
            None => "overlay".to_string(),
        };
        let problems = if node.problems.is_empty() {
            String::new()
        } else {
            format!("  ⚠ {}", node.problems.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
        };
        lines.push(format!("  {} {:?} {place}{problems}", node.widget_type, node.key));
    }
    if report.nodes.is_empty() {
        lines.push("  (none: give widgets a :key to see them here)".to_string());
    }
    if !report.overlaps.is_empty() {
        lines.push("Overlapping siblings:".to_string());
        lines.extend(report.overlaps.iter().map(|overlap| format!("  ⚠ {overlap}")));
    }
    lines.extend(hook_lines(&report.hooks));
    ToolOutput {
        content: vec![Content::Png(png), Content::Text(with_log(lines.join("\n"), &log))],
        is_error: false,
    }
}

/// A capture's `width` and `height` arguments, clamped, or the default size.
fn capture_size(args: &Json) -> (u32, u32) {
    let number = |key: &str, default: u32| {
        args.get(key).and_then(Json::as_u64).map(|n| n.clamp(64, 4096) as u32).unwrap_or(default)
    };
    (number("width", DEFAULT_CAPTURE_SIZE.0), number("height", DEFAULT_CAPTURE_SIZE.1))
}

/// The hooks a capture shows, at most [`HOOK_LINES`] per kind.
pub(crate) fn hook_lines(hooks: &eseqlisp::ui::capture::HookOverview) -> Vec<String> {
    let mut lines = Vec::new();
    let mut section = |title: &str, form: &str, uses: &[eseqlisp::ui::capture::HookUse]| {
        if uses.is_empty() {
            return;
        }
        lines.push(format!("  {title}: {form}"));
        for used in uses.iter().take(HOOK_LINES) {
            let times = if used.count > 1 { format!(" ×{}", used.count) } else { String::new() };
            lines.push(format!("    {}{times}  first at {}", used.name, used.first));
        }
        if uses.len() > HOOK_LINES {
            lines.push(format!("    … {} more", uses.len() - HOOK_LINES));
        }
    };
    section(
        "theme slots",
        "(override-theme-slot :slot \"widget\") restyles every use; :faces (dict :slot \"widget\") on a container, only the uses under it",
        &hooks.slots,
    );
    section("parts", "(restyle \"part\" |node| ...)", &hooks.parts);
    section("built by", "(override fn :around ...) on the function", &hooks.functions);
    if !lines.is_empty() {
        lines.insert(0, "Hooks to restyle it (what_draws x y names them at one spot):".to_string());
    }
    lines
}

const HOOK_LINES: usize = 12;

fn what_draws(
    session: &mut HeadlessSession,
    buffer: &str,
    size: (u32, u32),
    point: (f32, f32),
) -> ToolOutput {
    let (found, log) = capture_output(|| session.hooks_at(buffer, size, point));
    let stack = match found {
        Ok(stack) => stack,
        Err(problem) => return ToolOutput::error(with_log(problem, &log)),
    };
    ToolOutput::text(with_log(hooks_text(buffer, point, &stack), &log))
}

/// The widgets at `point` of `buffer`, topmost first, each with its hooks.
pub(crate) fn hooks_text(
    buffer: &str,
    point: (f32, f32),
    stack: &[eseqlisp::ui::capture::WidgetHooks],
) -> String {
    if stack.is_empty() {
        return format!("Nothing is drawn at {},{} in {buffer}.", point.0, point.1);
    }
    let mut lines = vec![format!("At {},{} in {buffer}, topmost first:", point.0, point.1)];
    let mut previous_builder = None;
    for hooks in stack {
        let key = hooks.key.as_deref().map(|key| format!(" {key:?}")).unwrap_or_default();
        lines.push(format!("{}{key} at {}", hooks.widget_type, hooks.region));
        for (slot, read_by) in &hooks.slots {
            lines.push(format!(
                "  theme slot :{slot} (read by {read_by}): (override-theme-slot :{slot} \"widget\"), or :faces (dict :{slot} \"widget\") on a container"
            ));
        }
        if let Some(face) = &hooks.face {
            lines.push(format!(
                "  painted by face {}: (restyle-face {:?} \"widget\"), or :faces on a container",
                face.painted_by, face.factory
            ));
        }
        if let Some(background) = &hooks.background {
            lines.push(format!("  background widget {background:?}"));
        }
        if let Some(part) = &hooks.part {
            lines.push(format!("  part: (restyle {part:?} |node| ...)"));
        }
        // Each builder once, at the first widget it built.
        if let Some(built_by) = hooks.built_by.as_ref().filter(|built_by| previous_builder != Some(*built_by)) {
            lines.push(format!("  built by {built_by}: (override {built_by} :around ...)"));
        }
        previous_builder = hooks.built_by.as_ref();
    }
    lines.join("\n")
}

/// Evaluate `code` and settle, as the live loop does after an evaluation
/// (host commands are applied even when the evaluation failed partway, so
/// none leak into the next call). The value is printed.
fn run_lisp(session: &mut HeadlessSession, code: &str) -> (Result<String, String>, String) {
    capture_output(|| {
        let runtime = session.editor.runtime_mut();
        let result = runtime.eval_str(code);
        let load_errors = runtime.take_source_load_errors();
        let printed = match result {
            Ok(value) => Ok(runtime.format_value(&value.unwrap_or(Value::Nil))),
            Err(VMError::ParseError) => Err(match forms::bracket_problem(code) {
                Some(problem) => format!("cannot parse the code: line {problem}"),
                None => "cannot parse the code".to_string(),
            }),
            Err(error) => Err(format!("{error:?}")),
        };
        let settled = session.settle();
        if !load_errors.is_empty() {
            return Err(load_errors.join("\n"));
        }
        printed.and_then(|value| settled.map(|()| value))
    })
}

fn lisp_tool_specs(session: &mut HeadlessSession) -> Vec<ToolSpec> {
    let (specs, _) = capture_output(|| {
        session.editor.runtime_mut().invoke_global(&format!("{REGISTRY_MODULE}/tool-specs"), Vec::new())
    });
    let Ok(Some(Value::List(specs))) = specs else { return Vec::new() };
    specs.iter().filter_map(|spec| lisp_spec(&spec.borrow())).collect()
}

/// `(dict :name "x" :doc "…" :params ((name :string "doc") (n :number "doc" :optional true)))`
/// as a tool spec.
fn lisp_spec(spec: &Value) -> Option<ToolSpec> {
    let Value::Map(map) = spec else { return None };
    let field = |key: &str| map.get(key).map(|cell| cell.borrow().clone());
    let Some(Value::String(name)) = field("name") else { return None };
    let doc = match field("doc") {
        Some(Value::String(doc)) => doc,
        _ => String::new(),
    };
    let mut params: Vec<(String, &str, String, bool)> = Vec::new();
    if let Some(Value::List(list)) = field("params") {
        for param in list {
            let Value::List(parts) = &*param.borrow() else { continue };
            let parts: Vec<Value> = parts.iter().map(|part| part.borrow().clone()).collect();
            let Some(Value::Symbol(pname) | Value::String(pname)) = parts.first().cloned() else { continue };
            let kind = match parts.get(1) {
                Some(Value::Keyword(kind)) => match kind.trim_start_matches(':') {
                    "number" => "number",
                    "integer" => "integer",
                    "bool" | "boolean" => "boolean",
                    _ => "string",
                },
                _ => "string",
            };
            let pdoc = match parts.get(2) {
                Some(Value::String(doc)) => doc.clone(),
                _ => String::new(),
            };
            let optional = parts.windows(2).any(|pair| {
                matches!(&pair[0], Value::Keyword(key) if key.trim_start_matches(':') == "optional")
                    && !matches!(pair[1], Value::Nil | Value::Bool(false))
            });
            params.push((pname, kind, pdoc, !optional));
        }
    }
    Some(ToolSpec {
        name,
        description: squash(&doc),
        input_schema: input_schema(params.iter().map(|(n, k, d, r)| (n.as_str(), *k, d.as_str(), *r))),
    })
}

fn input_schema<'a>(params: impl Iterator<Item = Param<'a>>) -> Json {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, kind, doc, needed) in params {
        properties.insert(name.to_string(), json!({ "type": kind, "description": doc }));
        if needed {
            required.push(name.to_string());
        }
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

fn call_lisp_tool(session: &mut HeadlessSession, name: &str, args: &Json) -> ToolOutput {
    let (result, log) = capture_output(|| {
        let runtime = session.editor.runtime_mut();
        let known = runtime.invoke_global(&format!("{REGISTRY_MODULE}/has-tool?"), vec![Value::String(name.to_string())]);
        if !matches!(known, Ok(Some(ref value)) if !matches!(value, Value::Nil | Value::Bool(false))) {
            return Err(format!("no tool `{name}`"));
        }
        let result = runtime.invoke_global(
            &format!("{REGISTRY_MODULE}/call-tool"),
            vec![Value::String(name.to_string()), json_to_value(args)],
        );
        let printed = match result {
            Ok(Some(Value::String(text))) => Ok(text),
            Ok(value) => Ok(runtime.format_value(&value.unwrap_or(Value::Nil))),
            Err(error) => Err(format!("{error:?}")),
        };
        let load_errors = runtime.take_source_load_errors();
        let settled = session.settle();
        if !load_errors.is_empty() {
            return Err(load_errors.join("\n"));
        }
        printed.and_then(|text| settled.map(|()| text))
    });
    match result {
        Ok(text) => ToolOutput::text(with_log(text, &log)),
        Err(problem) => ToolOutput::error(with_log(problem, &log)),
    }
}

/// JSON arguments as Lisp data: objects become maps keyed like `(dict :k v)`.
fn json_to_value(json: &Json) -> Value {
    let cell = |value| Rc::new(RefCell::new(value));
    match json {
        Json::Null => Value::Nil,
        Json::Bool(value) => Value::Bool(*value),
        Json::Number(number) => Value::Number(number.as_f64().unwrap_or(0.0)),
        Json::String(text) => Value::String(text.clone()),
        Json::Array(items) => Value::List(items.iter().map(|item| cell(json_to_value(item))).collect()),
        Json::Object(entries) => Value::Map(
            entries.iter().map(|(key, value)| (key.clone(), cell(json_to_value(value)))).collect::<HashMap<_, _>>(),
        ),
    }
}

/// `text`, then the log the call printed, if any (bounded).
fn with_log(text: String, log: &str) -> String {
    // The renderer announces shader reloads; they are not about the call.
    let log: String = log
        .lines()
        .filter(|line| !line.contains("shader-watch]"))
        .map(|line| format!("{line}\n"))
        .collect();
    let log = log.trim();
    if log.is_empty() {
        return text;
    }
    let shown: String = log.chars().take(MAX_LOG_CHARS).collect();
    let cut = if shown.len() < log.len() { "\n… (log truncated)" } else { "" };
    format!("{text}\n\nLog:\n{shown}{cut}")
}

/// Descriptions are written wrapped in source; send them as one paragraph.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}


/// `dgen_docs`: the DGen API catalog's entries for each query.
fn dgen_docs(query: &str, limit: usize) -> Result<ToolOutput, String> {
    thread_local! {
        static REGISTRY: Result<sequencer::agent::tools::AgentToolRegistry, String> =
            sequencer::agent::tools::AgentToolRegistry::load_default();
    }
    let queries: Vec<String> = query.split(',').map(str::trim).filter(|query| !query.is_empty()).map(str::to_string).collect();
    REGISTRY.with(|registry| {
        let registry = registry.as_ref().map_err(|why| format!("The DGen API catalog did not load: {why}"))?;
        Ok(ToolOutput::text(registry.lookup_dgen_docs(&queries, limit).content))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stop SIGKILLs the server mid-call, so no Drop runs: `mem::forget`
    /// stands in for that.
    fn killed_mid_call(journal: WriteJournal) {
        std::mem::forget(journal);
    }

    /// Rewrites the pid in every record, as if another server wrote them.
    fn written_by(dir: &Path, pid: u64) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let record = entry.unwrap().path();
            let mut body: Json = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
            body["pid"] = json!(pid);
            std::fs::write(&record, body.to_string()).unwrap();
        }
    }

    #[test]
    fn a_write_killed_mid_call_is_restored_by_the_next_server() {
        let root = tempfile::tempdir().unwrap();
        let journals = root.path().join("journals");
        let module = root.path().join("euclid.lisp");
        std::fs::write(&module, "(module euclid)\n(def ok () 1)\n").unwrap();

        let journal = WriteJournal::begin(&journals, &module, Some("(module euclid)\n(def ok () 1)\n")).unwrap();
        std::fs::write(&module, "(module euclid)\n(def broken () (nope))\n").unwrap();
        killed_mid_call(journal);

        let recovered = recover_journals(&journals);
        assert_eq!(recovered.len(), 1);
        assert_eq!((recovered[0].0.as_path(), recovered[0].1), (module.as_path(), true));
        assert_eq!(std::fs::read_to_string(&module).unwrap(), "(module euclid)\n(def ok () 1)\n");
        assert_eq!(std::fs::read_dir(&journals).unwrap().count(), 0, "the record is consumed");
        assert!(recover_journals(&journals).is_empty());
    }

    #[test]
    fn a_create_killed_before_its_import_finishes_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let journals = root.path().join("journals");
        let module = root.path().join("fresh.lisp");
        let journal = WriteJournal::begin(&journals, &module, None).unwrap();
        std::fs::write(&module, "(module fresh)\n").unwrap();
        killed_mid_call(journal);

        let recovered = recover_journals(&journals);
        assert_eq!(recovered.len(), 1);
        assert!(!recovered[0].1);
        assert!(!module.exists());
    }

    #[test]
    fn a_finished_call_leaves_no_record_and_a_live_servers_record_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let journals = root.path().join("journals");
        let module = root.path().join("m.lisp");
        std::fs::write(&module, "old").unwrap();
        drop(WriteJournal::begin(&journals, &module, Some("old")).unwrap());
        assert_eq!(std::fs::read_dir(&journals).unwrap().count(), 0);

        // The parent (the test runner) is alive and mid-call, as far as
        // the record says.
        let journal = WriteJournal::begin(&journals, &module, Some("old")).unwrap();
        std::fs::write(&module, "new").unwrap();
        killed_mid_call(journal);
        written_by(&journals, u64::from(std::os::unix::process::parent_id()));
        assert!(recover_journals(&journals).is_empty());
        assert_eq!(std::fs::read_to_string(&module).unwrap(), "new");
    }

    fn eval(runtime: &mut eseqlisp::Runtime, code: &str) -> Value {
        runtime.eval_str(code).unwrap().unwrap_or(Value::Nil)
    }

    #[test]
    fn a_lisp_tool_spec_becomes_a_tool_with_required_and_optional_params() {
        let mut runtime = eseqlisp::Runtime::new();
        let spec = eval(
            &mut runtime,
            "(dict :name \"rings\" :doc \"Ring lengths.\" \
               :params '((count :integer \"How many\") (label :string \"Name\" :optional true) (on :bool \"Lit\")))",
        );
        let spec = lisp_spec(&spec).unwrap();
        assert_eq!((spec.name.as_str(), spec.description.as_str()), ("rings", "Ring lengths."));
        assert_eq!(
            spec.input_schema,
            json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "description": "How many" },
                    "label": { "type": "string", "description": "Name" },
                    "on": { "type": "boolean", "description": "Lit" },
                },
                "required": ["count", "on"],
            })
        );
        assert!(lisp_spec(&eval(&mut runtime, "(dict :doc \"no name\")")).is_none());
    }

    #[test]
    fn json_arguments_read_in_lisp_like_a_dict() {
        let mut runtime = eseqlisp::Runtime::new();
        let args = json_to_value(&json!({ "bars": 2, "name": "kick", "steps": [1, 0], "on": true }));
        eval(&mut runtime, "(def read-args (args) (list args.bars (get args :name) (nth args.steps 1) args.on))");
        let read = runtime.invoke_global("read-args", vec![args]).unwrap().unwrap();
        assert_eq!(runtime.format_value(&read), "(2 \"kick\" 0 true)");
    }

    #[test]
    fn logs_drop_shader_reload_noise_and_stay_bounded() {
        let log = "[button-shader-watch] compiling\n[lisp-error][map] boom\n";
        assert_eq!(with_log("ok".to_string(), log), "ok\n\nLog:\n[lisp-error][map] boom");
        assert_eq!(with_log("ok".to_string(), "[x-shader-watch] only noise\n"), "ok");
        let long = "e".repeat(MAX_LOG_CHARS + 10);
        assert!(with_log("ok".to_string(), &long).ends_with("… (log truncated)"));
    }
}

#[cfg(test)]
mod coverage {
    use super::*;

    fn booted() -> HeadlessSession {
        let setup = sequencer::authoring_kit::SEQUENCER_CHECK_PROJECT;
        let parsed = super::super::super::capture::parse_capture_source(setup).unwrap();
        let script = std::env::temp_dir().join("eseq-doc-coverage.lisp");
        let mut session = HeadlessSession::boot(&script, &parsed, None, 0, false).unwrap();
        load_lisp_tools(&mut session);
        session
    }

    fn documented(runtime: &mut eseqlisp::Runtime, name: &str) -> bool {
        runtime.definition_header(name).and_then(|header| header.doc.clone()).is_some()
            || runtime.completion_metadata_for(name).is_some_and(|meta| !meta.docs.trim().is_empty())
    }

    /// The names the sequencer guide teaches an agent to call.
    const AGENT_SURFACE: &[&str] = &[
        "gen-tick", "gen-beat", "gen-bar", "gen-phase", "gen-rand", "gen-mark", "state-get", "state-set!",
        "seq-emit", "chan-get", "box", "h-stack", "v-stack", "label", "button", "number-picker", "dropdown",
        "sexp-slot", "each", "instance-ref", "describe-kind", "field", "set-field!", "field-info",
        "eseq.kinds/tracks", "eseq.kinds/groups", "eseq.kinds/generator-of", "eseq.kinds/generator-mark-of",
        "eseq.kinds/generator-mark-named", "eseq.view-kit/menu-of", "eseq.view-kit/open-menu!",
        "eseq.agent-tools/define-agent-tool",
    ];

    #[test]
    fn a_mute_set_from_eval_lands_in_the_headless_app() {
        let mut session = booted();
        let mute = "(import eseq.kinds :refer (tracks buses))\n\
                    (let ((t (nth (tracks) 1)) (b (first (buses)))) (set! t.muted true) (set! b.muted true))";
        run_lisp(&mut session, mute).0.unwrap();
        let (read, _) = run_lisp(
            &mut session,
            "(import eseq.kinds :refer (tracks buses))\n\
             (let ((t (nth (tracks) 1)) (b (first (buses)))) (list t.muted t.audible b.muted))",
        );
        assert_eq!(read.unwrap(), "(true false true)");
    }

    #[test]
    fn steps_toggled_from_eval_land_and_unknown_commands_are_named() {
        let mut session = booted();
        let on = "(import eseq.kinds :refer (track))\n\
                  (let ((t (track 0)) (s (nth t.steps 3))) (set! s.active true))\n\
                  (host-command \"toggle-step\" (dict :track 0 :step 5))";
        let (set, _) = run_lisp(&mut session, on);
        set.unwrap();
        let (read, _) = run_lisp(
            &mut session,
            "(import eseq.kinds :refer (track))\n\
             (let ((t (track 0)) (a (nth t.steps 3)) (b (nth t.steps 5))) (list a.active b.active (seq-track-step-active? 0 4)))",
        );
        assert_eq!(read.unwrap(), "(true true false)");
        let (_, log) = run_lisp(&mut session, "(host-command \"no-such-command\" (dict))");
        assert!(log.contains("Not applied in this headless copy") && log.contains("no-such-command"), "{log}");
    }

    #[test]
    fn every_name_the_sequencer_guide_teaches_is_documented() {
        let mut session = booted();
        let runtime = session.editor.runtime_mut();
        let missing: Vec<&str> =
            AGENT_SURFACE.iter().copied().filter(|name| !documented(runtime, name)).collect();
        assert!(missing.is_empty(), "describe has nothing for {missing:?}");
    }

    /// Run with `--run-ignored only --no-capture`: how much of the booted UI
    /// runtime `describe` / `apropos` can document, by module.
    #[test]
    #[ignore = "report, not a check"]
    fn doc_coverage_report() {
        let mut session = booted();
        let runtime = session.editor.runtime_mut();
        let mut by_module: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for name in runtime.callable_global_names() {
            let native = matches!(runtime.global_value(&name), Some(Value::NativeFunction(_)));
            let module = match name.rsplit_once('/') {
                Some((module, _)) if eseqlisp::modules::is_valid_module_name(module) => module.to_string(),
                // Per-instrument panel functions: generated, not API.
                _ if name.starts_with("custom_ui_") || name.starts_with("custom-synth-ui-") => continue,
                _ if native => "(native)".to_string(),
                _ => "(flat lisp)".to_string(),
            };
            let entry = by_module.entry(module).or_default();
            entry.0 += 1;
            entry.1 += usize::from(documented(runtime, &name));
        }
        let (mut total, mut docs) = (0, 0);
        for (module, (count, documented)) in &by_module {
            total += count;
            docs += documented;
            println!("{module:<48} {documented:>4}/{count:<4}");
        }
        println!("TOTAL {docs}/{total}");
    }
}
