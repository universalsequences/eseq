use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eseqlisp::vm::Value;

use sequencer::sample_db::{SampleDb, SampleRow, TagFacet};
use sequencer::app;

use super::current_custom_instrument_name;
use super::values::{build_icon_tree_items, list_value, map_value};

pub(crate) const SAMPLE_BROWSER_MAX_RESULTS: usize = 2000;

#[derive(Clone)]
pub(crate) struct SampleTreeNode {
    label: String,
    label_lower: String,
    path: Option<String>,
    children: Vec<SampleTreeNode>,
}

#[derive(Clone)]
pub(crate) struct InstrumentTreeNode {
    label: String,
    name: Option<String>,
    folder: Option<String>,
    children: Vec<InstrumentTreeNode>,
}

#[derive(Clone)]
pub(crate) struct ScriptTreeNode {
    label: String,
    path: Option<String>,
    children: Vec<ScriptTreeNode>,
}

struct BuiltinInstrumentDescriptor {
    label: &'static str,
    name: &'static str,
    icon: &'static str,
}

const BUILTIN_INSTRUMENTS: &[BuiltinInstrumentDescriptor] = &[
    BuiltinInstrumentDescriptor {
        label: "Sampler",
        name: "sampler",
        icon: "sampler",
    },
    BuiltinInstrumentDescriptor {
        label: "Modulator",
        name: "modulator",
        icon: "waveform",
    },
    BuiltinInstrumentDescriptor {
        label: "Drum Rack",
        name: "rack",
        icon: "sampler",
    },
    BuiltinInstrumentDescriptor {
        label: "Instrument Rack",
        name: "layer-rack",
        icon: "sampler",
    },
];

pub(crate) fn build_sample_tree_node(dir: &std::path::Path) -> Vec<SampleTreeNode> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push((name, path));
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("wav") {
                files.push((name, path.to_string_lossy().to_string()));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut items = Vec::new();

    for (label, path) in dirs {
        let children = build_sample_tree_node(&path);
        if children.is_empty() {
            continue;
        }
        items.push(SampleTreeNode {
            label_lower: label.to_lowercase(),
            label,
            path: None,
            children,
        });
    }

    for (label, full_path) in files {
        items.push(SampleTreeNode {
            label_lower: label.to_lowercase(),
            label,
            path: Some(full_path),
            children: Vec::new(),
        });
    }

    items
}

pub(crate) fn sample_tree_nodes_to_value(items: &[SampleTreeNode]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "label".to_string(),
                    Rc::new(RefCell::new(Value::String(item.label.clone()))),
                );
                if !item.children.is_empty() {
                    map.insert(
                        "children".to_string(),
                        Rc::new(RefCell::new(sample_tree_nodes_to_value(&item.children))),
                    );
                }
                if let Some(path) = &item.path {
                    map.insert(
                        "path".to_string(),
                        Rc::new(RefCell::new(Value::String(path.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

pub(crate) fn build_sample_tree_nodes_from_db(
    db: &SampleDb,
    query: &str,
    include_tags: &[&str],
    exclude_tags: &[&str],
) -> rusqlite::Result<Vec<SampleTreeNode>> {
    let query = query.trim();
    let rows = db.query(
        include_tags,
        exclude_tags,
        (!query.is_empty()).then_some(query),
        false,
    )?;
    let grouped = query.is_empty() && include_tags.is_empty() && exclude_tags.is_empty();
    Ok(sample_rows_to_tree_nodes(rows, grouped))
}

pub(crate) fn build_sample_tree_value_from_db(
    db: &SampleDb,
    query: &str,
    include_tags: &[&str],
    exclude_tags: &[&str],
) -> rusqlite::Result<Value> {
    let nodes = build_sample_tree_nodes_from_db(db, query, include_tags, exclude_tags)?;
    Ok(sample_tree_nodes_to_value(&nodes))
}

pub(crate) fn build_sample_browser_value_from_db(
    db: &SampleDb,
    query: &str,
    selected_tags: &[&str],
) -> rusqlite::Result<Value> {
    let query = query.trim();
    let has_active_filter =
        !query.is_empty() || selected_tags.iter().any(|tag| !tag.trim().is_empty());
    let tags = if has_active_filter {
        db.adjacent_tags(selected_tags, (!query.is_empty()).then_some(query), 32)?
    } else {
        default_sample_tag_facets(db, 16)?
    };
    let items = if has_active_filter {
        let rows = db.query_samples_for_browser_limited(
            selected_tags,
            (!query.is_empty()).then_some(query),
            SAMPLE_BROWSER_MAX_RESULTS,
        )?;
        sample_tree_nodes_to_value(&sample_rows_to_tree_nodes(rows, false))
    } else {
        Value::List(vec![])
    };
    Ok(map_value([
        ("tags", tag_facets_to_value(&tags)),
        ("items", items),
    ]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SampleBrowserRequest {
    query: String,
    selected_tags: Vec<String>,
}

impl SampleBrowserRequest {
    fn new(query: &str, selected_tags: &[&str]) -> Self {
        Self {
            query: query.trim().to_string(),
            selected_tags: selected_tags
                .iter()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect(),
        }
    }

    fn selected_tag_refs(&self) -> Vec<&str> {
        self.selected_tags.iter().map(String::as_str).collect()
    }
}

pub(crate) struct DebouncedSampleBrowser {
    db: Rc<SampleDb>,
    debounce: Duration,
    last_requested: Option<SampleBrowserRequest>,
    last_request_at: Option<Instant>,
    last_executed: Option<SampleBrowserRequest>,
    cached_value: Option<Value>,
    pending_text_query: bool,
}

impl DebouncedSampleBrowser {
    pub(crate) fn new(db: Rc<SampleDb>, debounce: Duration) -> Self {
        Self {
            db,
            debounce,
            last_requested: None,
            last_request_at: None,
            last_executed: None,
            cached_value: None,
            pending_text_query: false,
        }
    }

    pub(crate) fn query(&mut self, query: &str, selected_tags: &[&str]) -> rusqlite::Result<Value> {
        self.query_at(query, selected_tags, Instant::now())
    }

    pub(crate) fn poll_ready(&mut self) -> rusqlite::Result<bool> {
        let Some(request) = self.last_requested.clone() else {
            return Ok(false);
        };
        if !self.pending_text_query || self.last_executed.as_ref() == Some(&request) {
            return Ok(false);
        }
        let ready_at = self.last_request_at.unwrap_or_else(Instant::now) + self.debounce;
        if Instant::now() < ready_at {
            return Ok(false);
        }
        match self.execute_request(request) {
            Ok(_) => Ok(true),
            Err(error) => {
                self.pending_text_query = false;
                Err(error)
            }
        }
    }

    fn query_at(
        &mut self,
        query: &str,
        selected_tags: &[&str],
        now: Instant,
    ) -> rusqlite::Result<Value> {
        let request = SampleBrowserRequest::new(query, selected_tags);
        let previous_request = self.last_requested.as_ref();
        let request_changed = previous_request != Some(&request);
        let query_changed =
            previous_request.is_some_and(|previous| previous.query != request.query);

        if request_changed {
            self.pending_text_query = query_changed && !request.query.is_empty();
            self.last_requested = Some(request.clone());
            self.last_request_at = Some(now);
        }

        if self.last_executed.as_ref() == Some(&request) {
            if let Some(value) = &self.cached_value {
                return Ok(value.deep_clone());
            }
        }

        if self.pending_text_query {
            let ready_at = self.last_request_at.unwrap_or(now) + self.debounce;
            if now < ready_at {
                return Ok(self
                    .cached_value
                    .as_ref()
                    .map(Value::deep_clone)
                    .unwrap_or_else(empty_sample_browser_value));
            }
        }

        self.execute_request(request)
    }

    fn execute_request(&mut self, request: SampleBrowserRequest) -> rusqlite::Result<Value> {
        let selected_tag_refs = request.selected_tag_refs();
        let value =
            build_sample_browser_value_from_db(&self.db, &request.query, &selected_tag_refs)?;
        self.last_executed = Some(request);
        self.cached_value = Some(value.deep_clone());
        self.pending_text_query = false;
        Ok(value)
    }
}

fn empty_sample_browser_value() -> Value {
    map_value([
        ("tags", Value::List(vec![])),
        ("items", Value::List(vec![])),
    ])
}

fn default_sample_tag_facets(db: &SampleDb, max_tags: usize) -> rusqlite::Result<Vec<TagFacet>> {
    let global = db.adjacent_tags(&[], None, 256)?;
    let mut by_name: HashMap<String, TagFacet> = global
        .iter()
        .cloned()
        .map(|facet| (facet.name.to_lowercase(), facet))
        .collect();
    let mut tags = Vec::new();
    for name in DEFAULT_SAMPLE_BROWSER_TAGS {
        if let Some(facet) = by_name.remove(*name) {
            tags.push(facet);
        }
        if tags.len() >= max_tags {
            return Ok(tags);
        }
    }
    for facet in global {
        if tags
            .iter()
            .any(|tag| tag.name.eq_ignore_ascii_case(&facet.name))
        {
            continue;
        }
        tags.push(facet);
        if tags.len() >= max_tags {
            break;
        }
    }
    Ok(tags)
}

fn tag_facets_to_value(tags: &[TagFacet]) -> Value {
    list_value(tags.iter().map(|tag| {
        map_value([
            ("name", Value::String(tag.name.clone())),
            ("count", Value::Number(tag.count as f64)),
            ("selected", Value::Bool(tag.selected)),
        ])
    }))
}

const DEFAULT_SAMPLE_BROWSER_TAGS: &[&str] = &[
    "kick", "snare", "clap", "hat", "hi-hat", "break", "breaks", "bass", "keys", "synth", "pad",
    "vocals", "fx", "808", "909", "misc",
];

fn sample_rows_to_tree_nodes(rows: Vec<SampleRow>, grouped: bool) -> Vec<SampleTreeNode> {
    let mut leaves: Vec<(String, SampleTreeNode)> = rows
        .into_iter()
        .map(|row| {
            let label = row
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(&row.hash)
                .to_string();
            let category = primary_category(&row.tags);
            let node = SampleTreeNode {
                label_lower: label.to_lowercase(),
                label,
                path: Some(format!("samples/{}.wav", row.hash)),
                children: Vec::new(),
            };
            (category.to_string(), node)
        })
        .collect();

    leaves.sort_by(|a, b| {
        a.1.label_lower
            .cmp(&b.1.label_lower)
            .then_with(|| a.1.path.cmp(&b.1.path))
    });

    if !grouped {
        return leaves.into_iter().map(|(_, node)| node).collect();
    }

    let mut grouped_nodes = Vec::new();
    for category in SAMPLE_CATEGORY_ORDER {
        let children: Vec<SampleTreeNode> = leaves
            .iter()
            .filter(|(row_category, _)| row_category == category)
            .map(|(_, node)| node.clone())
            .collect();
        if children.is_empty() {
            continue;
        }
        grouped_nodes.push(SampleTreeNode {
            label: (*category).to_string(),
            label_lower: (*category).to_string(),
            path: None,
            children,
        });
    }
    grouped_nodes
}

const SAMPLE_CATEGORY_ORDER: &[&str] = &[
    "drums",
    "instruments",
    "manufacturers",
    "producers",
    "genres",
    "fx",
    "vocals",
    "hardware",
    "other",
];

fn primary_category(tags: &[String]) -> &'static str {
    for category in SAMPLE_CATEGORY_ORDER.iter().copied() {
        if category == "other" {
            continue;
        }
        if tags.iter().any(|tag| tag.eq_ignore_ascii_case(category)) {
            return category;
        }
    }
    "other"
}

fn build_instrument_tree_nodes(
    dir: &std::path::Path,
    root: &std::path::Path,
) -> Vec<InstrumentTreeNode> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push((name, path));
        } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
            let label = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if matches!(label.as_str(), "dsp" | "ui" | "presets") {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                let instrument_name = rel.with_extension("").to_string_lossy().replace('\\', "/");
                files.push((label, instrument_name));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut items = Vec::new();
    for (label, path) in dirs {
        if path.join("dsp.lisp").exists() {
            if let Ok(rel) = path.strip_prefix(root) {
                let instrument_name = format!("{}/", rel.to_string_lossy().replace('\\', "/"));
                items.push(InstrumentTreeNode {
                    label,
                    name: Some(instrument_name),
                    folder: None,
                    children: Vec::new(),
                });
            }
            continue;
        }

        let children = build_instrument_tree_nodes(&path, root);
        if !children.is_empty() {
            let folder = path
                .strip_prefix(root)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .filter(|folder| !folder.is_empty());
            items.push(InstrumentTreeNode {
                label,
                name: None,
                folder,
                children,
            });
        }
    }
    for (label, name) in files {
        items.push(InstrumentTreeNode {
            label,
            name: Some(name),
            folder: None,
            children: Vec::new(),
        });
    }
    items
}

fn instrument_tree_nodes_to_value(items: &[InstrumentTreeNode]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "label".to_string(),
                    Rc::new(RefCell::new(Value::String(item.label.clone()))),
                );
                if let Some(name) = &item.name {
                    map.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(name.clone()))),
                    );
                    map.insert(
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::String("instrument".to_string()))),
                    );
                    map.insert(
                        "icon".to_string(),
                        Rc::new(RefCell::new(Value::Keyword("piano".to_string()))),
                    );
                } else if let Some(folder) = &item.folder {
                    map.insert(
                        "folder".to_string(),
                        Rc::new(RefCell::new(Value::String(folder.clone()))),
                    );
                    map.insert(
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::String("folder".to_string()))),
                    );
                    map.insert(
                        "icon".to_string(),
                        Rc::new(RefCell::new(Value::Keyword("folder".to_string()))),
                    );
                }
                if !item.children.is_empty() {
                    map.insert(
                        "children".to_string(),
                        Rc::new(RefCell::new(instrument_tree_nodes_to_value(&item.children))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

fn tree_header(label: &'static str) -> Value {
    map_value([
        ("label", Value::String(label.to_string())),
        ("kind", Value::String("header".to_string())),
        ("draggable", Value::Bool(false)),
        ("drop-target", Value::Bool(false)),
    ])
}

fn builtin_instrument_matches(item: &BuiltinInstrumentDescriptor, query_lower: &str) -> bool {
    query_lower.is_empty()
        || item.label.to_lowercase().contains(query_lower)
        || item.name.contains(query_lower)
}

fn builtin_instrument_leaf(item: &BuiltinInstrumentDescriptor) -> Value {
    map_value([
        ("label", Value::String(item.label.to_string())),
        ("name", Value::String(item.name.to_string())),
        ("kind", Value::String("builtin-instrument".to_string())),
        ("icon", Value::Keyword(item.icon.to_string())),
        ("draggable", Value::Bool(item.name == "sampler")),
        ("drop-target", Value::Bool(false)),
    ])
}

fn builtin_instrument_values(query_lower: &str) -> Vec<Value> {
    BUILTIN_INSTRUMENTS
        .iter()
        .filter(|item| builtin_instrument_matches(item, query_lower))
        .map(builtin_instrument_leaf)
        .collect()
}

fn list_items(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items
            .into_iter()
            .map(|item| item.borrow().clone())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    }
}

fn append_tree_section(items: &mut Vec<Value>, label: &'static str, mut children: Vec<Value>) {
    if children.is_empty() {
        return;
    }
    items.push(tree_header(label));
    items.append(&mut children);
}

fn filter_instrument_tree_nodes(
    items: &[InstrumentTreeNode],
    query_lower: &str,
) -> Vec<InstrumentTreeNode> {
    if query_lower.is_empty() {
        return items.to_vec();
    }

    items
        .iter()
        .filter_map(|item| {
            let children = filter_instrument_tree_nodes(&item.children, query_lower);
            let label_matches = item.label.to_lowercase().contains(query_lower);
            let name_matches = item
                .name
                .as_ref()
                .map(|name| name.to_lowercase().contains(query_lower))
                .unwrap_or(false);
            if label_matches || name_matches || !children.is_empty() {
                let mut filtered = item.clone();
                filtered.children = children;
                Some(filtered)
            } else {
                None
            }
        })
        .collect()
}

fn project_engine_nodes(engine_names: &[String]) -> Vec<InstrumentTreeNode> {
    let mut seen = HashSet::new();
    engine_names
        .iter()
        .filter(|name| seen.insert((*name).clone()))
        .map(|name| InstrumentTreeNode {
            label: instrument_display_name(name),
            name: Some(name.clone()),
            folder: None,
            children: Vec::new(),
        })
        .collect()
}

pub(crate) fn build_instrument_tree_value(query: &str, project_engines: &[String]) -> Value {
    let query_lower = query.trim().to_lowercase();
    let root = std::path::Path::new("instruments");
    let top = build_instrument_tree_nodes(root, root);
    let custom = list_items(instrument_tree_nodes_to_value(
        &filter_instrument_tree_nodes(&top, &query_lower),
    ));
    let builtin = builtin_instrument_values(&query_lower);
    let engines = list_items(instrument_tree_nodes_to_value(
        &filter_instrument_tree_nodes(&project_engine_nodes(project_engines), &query_lower),
    ));

    let mut items = Vec::new();
    if query_lower.is_empty() {
        append_tree_section(&mut items, "Built-in", builtin);
        append_tree_section(&mut items, "Engines", engines);
        append_tree_section(&mut items, "Library", custom);
    } else {
        items.extend(builtin);
        items.extend(engines);
        items.extend(custom);
    }
    list_value(items)
}

pub(crate) fn project_instrument_engine_names(app: &app::App) -> Vec<String> {
    let mut engine_ids = Vec::new();
    for engine_id in app.graph.track_engine_ids.iter().flatten().copied() {
        if !engine_ids.contains(&engine_id) {
            engine_ids.push(engine_id);
        }
    }
    for engine_id in app
        .graph
        .track_node_ids
        .iter()
        .flat_map(|track| track.rack_slots.iter().filter_map(|slot| slot.engine_id))
    {
        if !engine_ids.contains(&engine_id) {
            engine_ids.push(engine_id);
        }
    }
    engine_ids
        .into_iter()
        .filter_map(|engine_id| {
            app.editor
                .engine_registry
                .get(engine_id)
                .map(|engine| engine.name.clone())
        })
        .collect()
}

pub(crate) fn script_root_dir() -> std::path::PathBuf {
    let local = std::path::PathBuf::from("scripts");
    if local.is_dir() {
        local
    } else {
        std::path::PathBuf::from("crates/sequencer/scripts")
    }
}

fn build_script_tree_nodes(dir: &std::path::Path, root: &std::path::Path) -> Vec<ScriptTreeNode> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push((name, path));
        } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let stable_path = path
                .strip_prefix(root)
                .ok()
                .map(|rel| script_root_dir().join(rel))
                .unwrap_or_else(|| path.clone())
                .to_string_lossy()
                .replace('\\', "/");
            files.push((label, stable_path));
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut items = Vec::new();
    for (label, path) in dirs {
        let children = build_script_tree_nodes(&path, root);
        if !children.is_empty() {
            items.push(ScriptTreeNode {
                label,
                path: None,
                children,
            });
        }
    }
    for (label, path) in files {
        items.push(ScriptTreeNode {
            label,
            path: Some(path),
            children: Vec::new(),
        });
    }
    items
}

fn script_tree_nodes_to_value(items: &[ScriptTreeNode]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "label".to_string(),
                    Rc::new(RefCell::new(Value::String(item.label.clone()))),
                );
                if let Some(path) = &item.path {
                    map.insert(
                        "path".to_string(),
                        Rc::new(RefCell::new(Value::String(path.clone()))),
                    );
                    map.insert(
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::String("script".to_string()))),
                    );
                } else {
                    map.insert(
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::String("folder".to_string()))),
                    );
                }
                if !item.children.is_empty() {
                    map.insert(
                        "children".to_string(),
                        Rc::new(RefCell::new(script_tree_nodes_to_value(&item.children))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

fn filter_script_tree_nodes(items: &[ScriptTreeNode], query_lower: &str) -> Vec<ScriptTreeNode> {
    if query_lower.is_empty() {
        return items.to_vec();
    }

    items
        .iter()
        .filter_map(|item| {
            let children = filter_script_tree_nodes(&item.children, query_lower);
            let label_matches = item.label.to_lowercase().contains(query_lower);
            let path_matches = item
                .path
                .as_ref()
                .map(|path| path.to_lowercase().contains(query_lower))
                .unwrap_or(false);
            if label_matches || path_matches || !children.is_empty() {
                let mut filtered = item.clone();
                filtered.children = children;
                Some(filtered)
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn build_script_tree(query: &str) -> Value {
    let query_lower = query.trim().to_lowercase();
    let root = script_root_dir();
    let top = build_script_tree_nodes(&root, &root);
    script_tree_nodes_to_value(&filter_script_tree_nodes(&top, &query_lower))
}

pub(crate) fn filter_sample_tree_nodes(
    items: &[SampleTreeNode],
    query_lower: &str,
) -> Vec<SampleTreeNode> {
    if query_lower.is_empty() {
        return items.to_vec();
    }

    let mut filtered = Vec::new();
    for item in items {
        if item.children.is_empty() {
            if item.label_lower.contains(query_lower) {
                filtered.push(item.clone());
            }
            continue;
        }

        let children = filter_sample_tree_nodes(&item.children, query_lower);
        if !children.is_empty() {
            filtered.push(SampleTreeNode {
                label: item.label.clone(),
                label_lower: item.label_lower.clone(),
                path: None,
                children,
            });
        }
    }
    filtered
}

pub(crate) fn build_project_tree(query: &str) -> Value {
    let query = query.trim().to_lowercase();
    let mut items = sequencer::project::list_project_entries().unwrap_or_default();
    items.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    if !query.is_empty() {
        items.retain(|item| item.name.to_lowercase().contains(&query));
    }
    list_value(items.into_iter().map(|item| {
        map_value([
            ("label", Value::String(item.name)),
            (
                "detail",
                Value::String(format_project_recency(item.modified_at)),
            ),
        ])
    }))
}

fn format_project_recency(modified_at: Option<SystemTime>) -> String {
    format_project_recency_at(SystemTime::now(), modified_at)
}

fn format_project_recency_at(now: SystemTime, modified_at: Option<SystemTime>) -> String {
    let Some(modified_at) = modified_at else {
        return String::new();
    };
    let Ok(age) = now.duration_since(modified_at) else {
        return "just now".to_string();
    };

    let minutes = age.as_secs() / 60;
    if minutes < 60 {
        return format!("{} min ago", minutes.max(1));
    }

    let hours = age.as_secs() / 3_600;
    if hours < 48 {
        let unit = if hours == 1 { "hour" } else { "hours" };
        return format!("{hours} {unit} ago");
    }

    let days = age.as_secs() / 86_400;
    if days < 30 {
        let unit = if days == 1 { "day" } else { "days" };
        return format!("{days} {unit} ago");
    }

    format_system_date(modified_at)
}

fn format_system_date(time: SystemTime) -> String {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let days = (duration.as_secs() / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

pub(crate) fn build_preset_tree_from_list(items_value: Option<&Value>, query: &str) -> Value {
    let query = query.trim().to_lowercase();
    let mut items: Vec<String> = match items_value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::String(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if !query.is_empty() {
        items.retain(|item| item.to_lowercase().contains(&query));
    }
    build_icon_tree_items(&items, "piano")
}

fn effect_leaf(label: String, kind: &'static str) -> Value {
    map_value([
        ("label", Value::String(label.clone())),
        ("name", Value::String(label)),
        ("kind", Value::String(kind.to_string())),
    ])
}

fn filter_effect_names(names: Vec<String>, query_lower: &str) -> Vec<String> {
    if query_lower.is_empty() {
        names
    } else {
        names
            .into_iter()
            .filter(|name| name.to_lowercase().contains(query_lower))
            .collect()
    }
}

fn build_audio_effect_tree_from_names(
    query: &str,
    builtin_names: Vec<String>,
    custom_names: Vec<String>,
) -> Value {
    let query_lower = query.trim().to_lowercase();
    let builtin: Vec<Value> = filter_effect_names(builtin_names, &query_lower)
        .into_iter()
        .map(|name| effect_leaf(name, "builtin-audio-effect"))
        .collect();

    let custom: Vec<Value> = filter_effect_names(custom_names, &query_lower)
        .into_iter()
        .map(|name| effect_leaf(name, "custom-audio-effect"))
        .collect();

    let mut items = Vec::new();
    if query_lower.is_empty() {
        append_tree_section(&mut items, "Built-in", builtin);
        append_tree_section(&mut items, "Custom", custom);
    } else {
        items.extend(builtin);
        items.extend(custom);
    }
    list_value(items)
}

pub(crate) fn build_audio_effect_tree(query: &str) -> Value {
    let mut builtin_names: Vec<String> =
        sequencer::effects::EffectDescriptor::builtin_insert_names()
            .iter()
            .map(|name| (*name).to_string())
            .collect();
    // DGenLisp-backed builtins are still presented through the builtin path.
    builtin_names.extend(
        sequencer::effects::dgen_builtin::NAMES
            .iter()
            .map(|name| (*name).to_string()),
    );
    build_audio_effect_tree_from_names(
        query,
        builtin_names,
        sequencer::lisp_host::list_saved_effects(),
    )
}

pub(crate) fn build_midi_effect_tree(query: &str) -> Value {
    let query_lower = query.trim().to_lowercase();
    let mut names: Vec<String> = sequencer::lisp_host::load_midi_fx_descriptors()
        .into_iter()
        .map(|desc| desc.name)
        .collect();
    names.sort();
    let items: Vec<Value> = filter_effect_names(names, &query_lower)
        .into_iter()
        .map(|name| effect_leaf(name, "midi-effect"))
        .collect();
    list_value(items)
}

pub(crate) fn instrument_display_name(name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(trimmed)
        .trim_end_matches(".lisp")
        .to_string()
}

pub(crate) fn visible_preset_items_for_track(app: &app::App, track: usize) -> Vec<String> {
    if app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack)
    {
        return sequencer::project::list_rack_presets().unwrap_or_default();
    }
    let Some(name) = current_custom_instrument_name(app, track) else {
        return Vec::new();
    };
    let mut items: Vec<String> = sequencer::lisp_host::load_instrument_presets(&name)
        .unwrap_or_default()
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    items.sort();
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_project_recency_windows() {
        let now = UNIX_EPOCH + Duration::from_secs(60 * 60 * 24 * 40);

        assert_eq!(
            format_project_recency_at(now, Some(now - Duration::from_secs(25 * 60))),
            "25 min ago"
        );
        assert_eq!(
            format_project_recency_at(now, Some(now - Duration::from_secs(5 * 60 * 60))),
            "5 hours ago"
        );
        assert_eq!(
            format_project_recency_at(now, Some(now - Duration::from_secs(12 * 86_400))),
            "12 days ago"
        );
        assert_eq!(
            format_project_recency_at(now, Some(UNIX_EPOCH)),
            "1970-01-01"
        );
    }

    fn sample_browser_db() -> Rc<SampleDb> {
        let db = Rc::new(SampleDb::open_in_memory().expect("open db"));
        db.connection()
            .execute(
                "INSERT INTO samples(hash, title) VALUES ('aaa111', 'Kick 808')",
                [],
            )
            .expect("sample");
        db.connection()
            .execute(
                "INSERT INTO samples(hash, title) VALUES ('bbb222', 'Kick 909')",
                [],
            )
            .expect("sample");
        db.connection()
            .execute(
                "INSERT INTO samples(hash, title) VALUES ('ccc333', 'Snare')",
                [],
            )
            .expect("sample");
        db.add_tag("aaa111", "kick").expect("tag");
        db.add_tag("aaa111", "808").expect("tag");
        db.add_tag("bbb222", "kick").expect("tag");
        db.add_tag("bbb222", "909").expect("tag");
        db.add_tag("ccc333", "snare").expect("tag");
        db
    }

    fn sample(hash: &str, title: Option<&str>, tags: &[&str]) -> SampleRow {
        SampleRow {
            hash: hash.to_string(),
            title: title.map(str::to_string),
            favorited: false,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
        }
    }

    fn sample_browser_item_labels(value: &Value) -> Vec<String> {
        let Value::Map(result) = value else {
            panic!("browser result should be a map");
        };
        let items = result.get("items").expect("items").borrow();
        let Value::List(items) = &*items else {
            panic!("items should be a list");
        };
        items
            .iter()
            .filter_map(|item| {
                let item = item.borrow();
                let Value::Map(map) = &*item else {
                    return None;
                };
                map.get("label").and_then(|label| match &*label.borrow() {
                    Value::String(label) => Some(label.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    fn top_level_tree_labels(value: &Value) -> Vec<String> {
        let Value::List(items) = value else {
            panic!("tree should be a list");
        };
        items
            .iter()
            .filter_map(|item| {
                let item = item.borrow();
                let Value::Map(map) = &*item else {
                    return None;
                };
                map.get("label").and_then(|label| match &*label.borrow() {
                    Value::String(label) => Some(label.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    #[test]
    fn instrument_tree_places_unique_project_engines_between_builtins_and_library() {
        let tree = build_instrument_tree_value(
            "",
            &["drums/3d-drum/".to_string(), "drums/3d-drum/".to_string()],
        );
        let labels = top_level_tree_labels(&tree);

        assert_eq!(
            &labels[..7],
            &[
                "Built-in",
                "Sampler",
                "Modulator",
                "Drum Rack",
                "Instrument Rack",
                "Engines",
                "3d-drum",
            ]
        );
        assert_eq!(labels.iter().filter(|label| *label == "3d-drum").count(), 1);
    }

    #[test]
    fn instrument_tree_assigns_piano_and_folder_icons() {
        let value = instrument_tree_nodes_to_value(&[
            InstrumentTreeNode {
                label: "Folder".to_string(),
                name: None,
                folder: Some("folder".to_string()),
                children: Vec::new(),
            },
            InstrumentTreeNode {
                label: "My Instrument".to_string(),
                name: Some("folder/my-instrument/".to_string()),
                folder: None,
                children: Vec::new(),
            },
        ]);
        let Value::List(items) = value else {
            panic!("instrument tree should be a list");
        };
        let folder = items[0].borrow();
        let instrument = items[1].borrow();
        let Value::Map(folder) = &*folder else {
            panic!("folder should be a map");
        };
        let Value::Map(instrument) = &*instrument else {
            panic!("instrument should be a map");
        };

        assert_eq!(
            folder.get("icon").map(|value| value.borrow().clone()),
            Some(Value::Keyword("folder".to_string()))
        );
        assert_eq!(
            instrument.get("icon").map(|value| value.borrow().clone()),
            Some(Value::Keyword("piano".to_string()))
        );
    }

    #[test]
    fn preset_tree_assigns_piano_icons_to_filtered_rows() {
        let presets = list_value([
            Value::String("Brutal Fifths".to_string()),
            Value::String("Galactic Pad".to_string()),
        ]);
        let tree = build_preset_tree_from_list(Some(&presets), "brutal");
        let Value::List(items) = tree else {
            panic!("preset tree should be a list");
        };
        assert_eq!(items.len(), 1);
        let item = items[0].borrow();
        let Value::Map(item) = &*item else {
            panic!("preset row should be a map");
        };

        assert_eq!(
            item.get("icon").map(|value| value.borrow().clone()),
            Some(Value::Keyword("piano".to_string()))
        );
    }

    #[test]
    fn audio_effect_tree_omits_empty_custom_header() {
        let tree = build_audio_effect_tree_from_names("", vec!["EQ8".to_string()], Vec::new());
        assert_eq!(top_level_tree_labels(&tree), vec!["Built-in", "EQ8"]);
    }

    #[test]
    fn db_sample_tree_groups_by_primary_category_with_other_fallback() {
        let nodes = sample_rows_to_tree_nodes(
            vec![
                sample("ccc333", Some("Pad"), &["instruments"]),
                sample("aaa111", Some("Kick"), &["drums", "dark"]),
                sample("bbb222", Some("Texture"), &["weird"]),
            ],
            true,
        );

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].label, "drums");
        assert_eq!(nodes[0].children[0].label, "Kick");
        assert_eq!(nodes[1].label, "instruments");
        assert_eq!(nodes[1].children[0].label, "Pad");
        assert_eq!(nodes[2].label, "other");
        assert_eq!(
            nodes[2].children[0].path.as_deref(),
            Some("samples/bbb222.wav")
        );
    }

    #[test]
    fn db_sample_tree_flattened_results_use_hash_when_title_is_empty() {
        let nodes = sample_rows_to_tree_nodes(
            vec![
                sample("bbb222", Some("  "), &["drums"]),
                sample("aaa111", Some("Alpha"), &["drums"]),
            ],
            false,
        );

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].label, "Alpha");
        assert_eq!(nodes[0].path.as_deref(), Some("samples/aaa111.wav"));
        assert_eq!(nodes[1].label, "bbb222");
        assert_eq!(nodes[1].path.as_deref(), Some("samples/bbb222.wav"));
    }

    #[test]
    fn db_sample_browser_returns_flat_items_and_adjacent_tag_chips() {
        let db = sample_browser_db();

        let Value::Map(result) =
            build_sample_browser_value_from_db(&db, "", &["kick"]).expect("browser")
        else {
            panic!("browser result should be a map");
        };
        let tags = result.get("tags").expect("tags").borrow();
        let Value::List(tags) = &*tags else {
            panic!("tags should be a list");
        };
        let tag_names: Vec<String> = tags
            .iter()
            .filter_map(|tag| {
                let tag = tag.borrow();
                let Value::Map(map) = &*tag else {
                    return None;
                };
                map.get("name").and_then(|name| match &*name.borrow() {
                    Value::String(name) => Some(name.clone()),
                    _ => None,
                })
            })
            .collect();
        assert!(tag_names.contains(&"kick".to_string()));
        assert!(tag_names.contains(&"808".to_string()));
        assert!(tag_names.contains(&"909".to_string()));

        let items = result.get("items").expect("items").borrow();
        let Value::List(items) = &*items else {
            panic!("items should be a list");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn debounced_sample_browser_waits_for_stable_text_query() {
        let db = sample_browser_db();
        let mut browser = DebouncedSampleBrowser::new(db.clone(), Duration::from_millis(100));
        let start = Instant::now();

        let initial = browser.query_at("", &[], start).expect("initial browser");
        assert!(sample_browser_item_labels(&initial).is_empty());

        let pending_first_char = browser
            .query_at("k", &[], start + Duration::from_millis(10))
            .expect("pending first char");
        assert!(
            sample_browser_item_labels(&pending_first_char).is_empty(),
            "first text change should return cached browser state before querying"
        );

        let pending_second_char = browser
            .query_at("ki", &[], start + Duration::from_millis(50))
            .expect("pending second char");
        assert!(
            sample_browser_item_labels(&pending_second_char).is_empty(),
            "new text should restart the debounce window"
        );

        let ready = browser
            .query_at("ki", &[], start + Duration::from_millis(151))
            .expect("debounced query");
        assert_eq!(
            sample_browser_item_labels(&ready),
            vec!["Kick 808".to_string(), "Kick 909".to_string()]
        );
    }

    #[test]
    fn debounced_sample_browser_poll_ready_executes_pending_query() {
        let db = sample_browser_db();
        let mut browser = DebouncedSampleBrowser::new(db.clone(), Duration::from_millis(1));
        let start = Instant::now();

        browser.query_at("", &[], start).expect("initial browser");
        let pending = browser
            .query_at("ki", &[], start + Duration::from_millis(1))
            .expect("pending query");
        assert!(sample_browser_item_labels(&pending).is_empty());

        std::thread::sleep(Duration::from_millis(2));
        assert!(
            browser.poll_ready().expect("poll pending query"),
            "poll_ready should execute a matured pending text query"
        );
        let cached = browser
            .query_at("ki", &[], start + Duration::from_millis(2))
            .expect("cached debounced query");
        assert_eq!(
            sample_browser_item_labels(&cached),
            vec!["Kick 808".to_string(), "Kick 909".to_string()]
        );
    }

    #[test]
    fn debounced_sample_browser_applies_tag_changes_immediately() {
        let db = sample_browser_db();
        let mut browser = DebouncedSampleBrowser::new(db.clone(), Duration::from_millis(100));
        let start = Instant::now();

        browser.query_at("", &[], start).expect("initial browser");
        let tagged = browser
            .query_at("", &["kick"], start + Duration::from_millis(1))
            .expect("tagged browser");

        assert_eq!(
            sample_browser_item_labels(&tagged),
            vec!["Kick 808".to_string(), "Kick 909".to_string()]
        );
    }

    #[test]
    fn db_sample_browser_caps_materialized_results() {
        let db = Rc::new(SampleDb::open_in_memory().expect("open db"));
        for idx in 0..(SAMPLE_BROWSER_MAX_RESULTS + 25) {
            let hash = format!("sample{idx:03}");
            let title = format!("Kick {idx:03}");
            db.connection()
                .execute(
                    "INSERT INTO samples(hash, title) VALUES (?, ?)",
                    rusqlite::params![hash, title],
                )
                .expect("sample");
            db.add_tag(&hash, "kick").expect("tag");
        }

        let labels = sample_browser_item_labels(
            &build_sample_browser_value_from_db(&db, "", &["kick"]).expect("browser"),
        );
        assert_eq!(labels.len(), SAMPLE_BROWSER_MAX_RESULTS);
    }
}
