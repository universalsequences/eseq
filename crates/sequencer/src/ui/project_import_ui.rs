//! The browser's Projects peek (docs/rig-spec.md §8, "Browse"): another
//! project read-only as a tree, and the host half of importing from it.
//!
//! `(seq-project-peek name-or-path)` parses the file once and caches the
//! tree it hands Lisp (and a failure, as an `:error` map), keyed by path and
//! modification time, so the view can ask every frame;
//! `(seq-project-peek-close)` drops the cache (the parsed file is large).
//! A parse runs on the UI thread: a ~6 MB project takes a moment the first
//! time it is peeked (no background parse yet). `import-from-project` runs
//! the App's import with the picks the view collected and finishes the kits
//! and sequencers it brought on the host side inside the same undo step.
//! `(seq-project-imported name)` lists what this session imported from it.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use eseqlisp::vm::Value;
use eseqlisp::Runtime;
use sequencer::app::project_import::{
    ImportRequest, ImportSource, ImportTarget, PeekDevice, PeekKit, PickKind, ProjectPeek,
};

use crate::*;

/// The peeked project: its parsed source (or why it failed) and the tree
/// value Lisp reads.
struct CachedSource {
    path: PathBuf,
    modified: Option<SystemTime>,
    source: Result<Rc<ImportSource>, String>,
    value: Value,
}

thread_local! {
    static PEEKED: RefCell<Option<CachedSource>> = const { RefCell::new(None) };
    /// What this session imported from each source file: pick-kind name
    /// (`"kit"`, `"bank"`, ...) to ids, the peek's checkmarks.
    static IMPORTED: RefCell<HashMap<PathBuf, HashMap<&'static str, BTreeSet<u64>>>> =
        RefCell::new(HashMap::new());
}

/// An import the view asked for (`EditSessionState::pending_project_import`).
pub(crate) struct PendingImport {
    payload: Value,
    /// Set once a loop turn has passed (and drawn the loading toast).
    armed: bool,
}

/// The pick kinds by the names the view's `:picks` dict uses.
const PICK_KINDS: [(PickKind, &str); 5] = [
    (PickKind::Kit, "kit"),
    (PickKind::Group, "group"),
    (PickKind::Track, "track"),
    (PickKind::Bank, "bank"),
    (PickKind::Sequencer, "sequencer"),
];

/// A project name (as the Projects tab lists it) or a file path.
pub(crate) fn project_path(name_or_path: &str) -> PathBuf {
    let candidate = Path::new(name_or_path);
    if candidate.is_absolute() || name_or_path.ends_with(".json") || name_or_path.contains('/') {
        return candidate.to_path_buf();
    }
    sequencer::project::project_file_path(name_or_path)
}

/// Run `read` on the cached source at `path`, parsing it first when the
/// cache holds another file or an older version of this one.
fn with_source<T>(path: &Path, read: impl FnOnce(&CachedSource) -> T) -> T {
    let modified = std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    PEEKED.with(|cell| {
        let mut cell = cell.borrow_mut();
        let fresh = cell
            .as_ref()
            .is_some_and(|cached| cached.path == path && cached.modified == modified);
        if !fresh {
            let source = ImportSource::open(path).map(Rc::new);
            let value = match &source {
                Ok(source) => peek_value(&source.peek(), path),
                Err(error) => map_value([("error", string(error.clone()))]),
            };
            *cell = Some(CachedSource { path: path.to_path_buf(), modified, source, value });
        }
        read(cell.as_ref().expect("cached above"))
    })
}

fn number(n: impl Into<f64>) -> Value {
    Value::Number(n.into())
}

fn string(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn with_banks(mut text: String, banks: &[String]) -> String {
    match banks {
        [] => text.push_str(" · no scenes"),
        [one] => text.push_str(&format!(" · bank {one}")),
        many => text.push_str(&format!(" · banks {}", many.join(", "))),
    }
    text
}

/// A pad line: `{:pad :kind :label :slots}` (`:kind` "sample", "synth",
/// "modulator" or "empty").
fn device_value(device: &PeekDevice) -> Value {
    map_value([
        ("pad", string(device.pad.clone())),
        ("kind", string(device.kind.name())),
        ("label", string(device.label.clone())),
        ("slots", number(device.slots as f64)),
    ])
}

fn kit_detail(kit: &PeekKit) -> String {
    let mut text = format!("{} · {}", plural(kit.pads, "pad"), plural(kit.clips, "clip"));
    if !kit.sequencers.is_empty() {
        text.push_str(&format!(" · {}", kit.sequencers.join(", ")));
    }
    with_banks(text, &kit.banks)
}

/// The peek as Lisp data. Every row carries `:id`, `:name` (what a pick
/// is checked against at import) and a one-line `:detail`.
fn peek_value(peek: &ProjectPeek, path: &Path) -> Value {
    let row = |id: f64, name: String, detail: String, extra: Vec<(&'static str, Value)>| {
        let mut entries = vec![("id", number(id)), ("name", string(name)), ("detail", string(detail))];
        entries.extend(extra);
        map_value(entries)
    };
    map_value([
        ("name", string(peek.name.clone())),
        ("path", string(path.to_string_lossy().to_string())),
        ("version", number(peek.version)),
        (
            "banks",
            list_value(peek.banks.iter().map(|bank| {
                row(bank.id as f64, bank.label.clone(), plural(bank.scenes.len(), "scene"), Vec::new())
            })),
        ),
        (
            "kits",
            list_value(peek.kits.iter().map(|kit| {
                row(kit.group_id as f64, kit.name.clone(), kit_detail(kit), vec![("devices", list_value(kit.devices.iter().map(device_value)))])
            })),
        ),
        (
            "groups",
            list_value(peek.groups.iter().map(|group| {
                row(group.group_id as f64, group.name.clone(), plural(group.tracks.len(), "track"), Vec::new())
            })),
        ),
        (
            "tracks",
            list_value(peek.tracks.iter().map(|track| {
                row(
                    track.index as f64,
                    track.name.clone(),
                    with_banks(track.device.clone(), &track.banks),
                    Vec::new(),
                )
            })),
        ),
        (
            "sequencers",
            list_value(peek.sequencers.iter().map(|sequencer| {
                row(sequencer.id as f64, sequencer.label.clone(), sequencer.kind.clone(), Vec::new())
            })),
        ),
    ])
}

pub(crate) fn register_project_import_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "seq-project-peek",
        "(seq-project-peek name-or-path)",
        "Another project read-only (rig spec §8): its banks, kits, groups, tracks and \
         project sequencers, or a map with :error.",
        |args, _ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Ok(map_value([("error", string("seq-project-peek takes a project name"))]));
            };
            Ok(with_source(&project_path(name), |cached| cached.value.clone()))
        },
    );
    runtime.register_native_with_docs(
        "seq-project-peek-close",
        "(seq-project-peek-close)",
        "Forget the peeked project (its parsed file is large).",
        |_args, _ctx| {
            PEEKED.with(|cell| *cell.borrow_mut() = None);
            Ok(Value::Nil)
        },
    );
    runtime.register_native_with_docs(
        "seq-project-imported",
        "(seq-project-imported name-or-path)",
        "What this session imported from that project, by pick kind: \
         {:kit (ids) :bank (ids) ...} (the peek's checkmarks).",
        |args, _ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Ok(map_value(std::iter::empty()));
            };
            let path = project_path(name);
            Ok(IMPORTED.with(|imported| {
                let imported = imported.borrow();
                let kinds = imported.get(&path);
                map_value(kinds.into_iter().flatten().map(|(kind, ids)| {
                    (*kind, list_value(ids.iter().map(|id| number(*id as f64))))
                }))
            }))
        },
    );
}

/// The picks of kind `kind` in the payload's `:picks` dict: a list of
/// `{:id :name}` maps.
fn picks(payload: &Value, kind: &str) -> Vec<(u64, String)> {
    let Value::Map(map) = payload else {
        return Vec::new();
    };
    let Some(picks) = map.get("picks") else {
        return Vec::new();
    };
    let Value::Map(picks) = &*picks.borrow() else {
        return Vec::new();
    };
    let Some(list) = picks.get(kind) else {
        return Vec::new();
    };
    let Value::List(items) = &*list.borrow() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Map(entry) = &*item.borrow() else {
                return None;
            };
            let id = match &*entry.get("id")?.borrow() {
                Value::Number(n) if *n >= 0.0 => *n as u64,
                _ => return None,
            };
            let name = match entry.get("name").map(|cell| cell.borrow().clone()) {
                Some(Value::String(name)) => name,
                _ => String::new(),
            };
            Some((id, name))
        })
        .collect()
}

/// `import-from-project {:name n :picks {:kit ({:id :name} ...) :group
/// :track :bank :sequencer} :target :bank|:new-bank :bank-id b}`: shows a
/// loading toast and queues the import; [`run_pending_import`] runs it once
/// a frame has drawn the toast.
pub(crate) fn handle_import_from_project(payload: Value, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    if ctx.sessions.pending_project_import.is_some() {
        editor.show_toast("An import is already running", eseqlisp::ToastKind::Error);
        return;
    }
    let names: Vec<String> = PICK_KINDS
        .iter()
        .flat_map(|(_, key)| picks(&payload, key).into_iter().map(|(_, name)| name))
        .collect();
    editor.show_loading_toast(format!("Importing {}…", pick_names(&names)), None);
    ctx.sessions.pending_project_import = Some(PendingImport { payload, armed: false });
}

/// Once per loop turn: runs the queued import on the turn after it was
/// asked for (the first turn only arms it, so the loading toast draws).
pub(crate) fn run_pending_import(app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let Some(pending) = ctx.sessions.pending_project_import.as_mut() else {
        return;
    };
    if !pending.armed {
        pending.armed = true;
        return;
    }
    if let Some(pending) = ctx.sessions.pending_project_import.take() {
        run_import(pending.payload, app, editor, ctx);
    }
}

/// "Group 1", "Group 1 and Drum Rack 2", "Group 1 and 2 more".
fn pick_names(names: &[String]) -> String {
    match names {
        [] => "picks".to_string(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
    }
}

fn run_import(payload: Value, app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let fail = |editor: &mut Editor, error: String| {
        editor.handle_host_event(HostEvent::Status(format!("Import failed: {error}")));
        editor.show_toast(format!("Import failed: {error}"), eseqlisp::ToastKind::Error);
    };
    let Some(name) = extract_string_from_payload(&payload, "path")
        .or_else(|| extract_string_from_payload(&payload, "name"))
    else {
        fail(editor, "no project".to_string());
        return;
    };
    let path = project_path(&name);
    let source = match with_source(&path, |cached| cached.source.clone()) {
        Ok(source) => source,
        Err(error) => {
            fail(editor, error);
            return;
        }
    };
    let target = match &payload {
        Value::Map(map) => map.get("target").map(|cell| cell.borrow().clone()),
        _ => None,
    };
    let target = match target {
        Some(Value::Keyword(keyword)) if keyword == "new-bank" => ImportTarget::NewBank,
        _ => ImportTarget::Bank(
            extract_usize_from_payload(&payload, "bank-id")
                .filter(|id| *id > 0)
                .map(|id| sequencer::sequencer::SceneBankId(id as u64)),
        ),
    };
    let mut request = ImportRequest { target, ..Default::default() };
    let mut picked = Vec::new();
    for (kind, key) in PICK_KINDS {
        for (id, name) in picks(&payload, key) {
            match kind {
                PickKind::Kit => request.kits.push(id),
                PickKind::Group => request.groups.push(id),
                PickKind::Track => request.tracks.push(id as usize),
                PickKind::Bank => request.banks.push(id),
                PickKind::Sequencer => request.sequencers.push(id),
            }
            picked.push((key, id, name.clone()));
            request.expected_names.push((kind, id, name));
        }
    }
    let tracks_before = app.tracks.len();
    let result = app.import_from_project_then(&source, &request, |app, summary| {
        let focus = (app.tracks.len() > tracks_before).then_some(tracks_before);
        let groups: Vec<u64> = summary.groups.iter().map(|(_, new)| *new).collect();
        let instances: Vec<u64> = summary.instances.iter().map(|(_, new)| *new).collect();
        super::host_commands::finish_imported_racks(app, editor, ctx, &groups, &instances, focus)
    });
    match result {
        Ok(summary) => {
            IMPORTED.with(|imported| {
                let mut imported = imported.borrow_mut();
                let kinds = imported.entry(path).or_default();
                for (key, id, _) in &picked {
                    kinds.entry(*key).or_default().insert(*id);
                }
            });
            editor.handle_host_event(HostEvent::Status(summary.message()));
            // A new bank is where the import is on: view it (once the host
            // lists it), as the transport's "New bank" does.
            if summary.new_bank {
                if let Some(position) = summary.bank.and_then(|bank| {
                    app.state.scene_banks().iter().position(|candidate| candidate.id == bank)
                }) {
                    let form = format!("(eseq.scene-banks/view-scene-bank-at! {position})");
                    if let Err(error) = editor.runtime_mut().eval_str(&form) {
                        eprintln!("metal_seq: could not view the imported bank: {error:?}");
                    }
                }
            }
            let names: Vec<String> = picked.into_iter().map(|(_, _, name)| name).collect();
            let mut toast = format!(
                "Imported {} into {}bank {}",
                pick_names(&names),
                if summary.new_bank { "new " } else { "" },
                summary.bank_label
            );
            if !summary.unconnected.is_empty() {
                toast.push_str(&format!(" · {} left unconnected", plural(summary.unconnected.len(), "link")));
            }
            editor.show_toast(toast, eseqlisp::ToastKind::Success);
        }
        Err(error) => fail(editor, error),
    }
}
