use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use eseqlisp::vm::Value;

use sequencer::ui;

use super::current_custom_instrument_name;
use super::values::build_flat_tree_items;

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
                    children: Vec::new(),
                });
            }
            continue;
        }

        let children = build_instrument_tree_nodes(&path, root);
        if !children.is_empty() {
            items.push(InstrumentTreeNode {
                label,
                name: None,
                children,
            });
        }
    }
    for (label, name) in files {
        items.push(InstrumentTreeNode {
            label,
            name: Some(name),
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
    let mut items: Vec<String> = sequencer::lisp_effect::load_instrument_presets(&name)
        .unwrap_or_default()
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    items.sort();
    items
}
