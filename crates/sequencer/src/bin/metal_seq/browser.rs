use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eseqlisp::vm::Value;

use sequencer::sample_db::{SampleDb, SampleRow, TagFacet};
use sequencer::ui;

use super::current_custom_instrument_name;
use super::values::{build_flat_tree_items, list_value, map_value};

pub(crate) const SAMPLE_BROWSER_MAX_RESULTS: usize = 200;

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
                } else if let Some(folder) = &item.folder {
                    map.insert(
                        "folder".to_string(),
                        Rc::new(RefCell::new(Value::String(folder.clone()))),
                    );
                    map.insert(
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::String("folder".to_string()))),
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

pub(crate) fn build_instrument_tree_value(query: &str) -> Value {
    let query_lower = query.trim().to_lowercase();
    let root = std::path::Path::new("instruments");
    let top = build_instrument_tree_nodes(root, root);
    let top = filter_instrument_tree_nodes(&top, &query_lower);

    instrument_tree_nodes_to_value(&top)
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

fn visible_project_items() -> Vec<String> {
    sequencer::project::list_project_names().unwrap_or_default()
}

pub(crate) fn build_project_tree(query: &str) -> Value {
    let query = query.trim().to_lowercase();
    let mut items = visible_project_items();
    if !query.is_empty() {
        items.retain(|item| item.to_lowercase().contains(&query));
    }
    build_flat_tree_items(&items)
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
    build_flat_tree_items(&items)
}

fn effect_leaf(label: String, kind: &'static str) -> Value {
    map_value([
        ("label", Value::String(label.clone())),
        ("name", Value::String(label)),
        ("kind", Value::String(kind.to_string())),
    ])
}

fn effect_section(label: &'static str, children: Vec<Value>) -> Option<Value> {
    if children.is_empty() {
        None
    } else {
        Some(map_value([
            ("label", Value::String(label.to_string())),
            ("kind", Value::String("section".to_string())),
            ("children", list_value(children)),
        ]))
    }
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

pub(crate) fn build_audio_effect_tree(query: &str) -> Value {
    let query_lower = query.trim().to_lowercase();
    let mut builtin_names: Vec<String> =
        sequencer::effects::EffectDescriptor::builtin_insert_names()
            .iter()
            .map(|name| (*name).to_string())
            .collect();
    // dgenlisp-backed builtins (DSP body is dgenlisp, but added via the builtin path)
    builtin_names.push(sequencer::conv_reverb::NAME.to_string());
    let builtin: Vec<Value> = filter_effect_names(builtin_names, &query_lower)
        .into_iter()
        .map(|name| effect_leaf(name, "builtin-audio-effect"))
        .collect();

    let custom: Vec<Value> =
        filter_effect_names(sequencer::lisp_host::list_saved_effects(), &query_lower)
            .into_iter()
            .map(|name| effect_leaf(name, "custom-audio-effect"))
            .collect();

    let mut sections = Vec::new();
    if let Some(section) = effect_section("Built-in", builtin) {
        sections.push(section);
    }
    if let Some(section) = effect_section("Custom", custom) {
        sections.push(section);
    }
    list_value(sections)
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

pub(crate) fn visible_preset_items_for_track(app: &ui::App, track: usize) -> Vec<String> {
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
