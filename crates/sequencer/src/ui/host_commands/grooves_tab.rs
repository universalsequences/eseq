//! The Grooves browser tab (docs/rack-groove-spec.md, "Rev 2 UI"; bead
//! eseq-groove.11): a tree beside Packages with the project groove pool and
//! the groove library, in the Packages tab's conventions (section headers,
//! `:status-icon`, `:badge`, right-click menus routed to host commands).
//!
//! `(seq-groove-tree query pool library)` turns the published
//! `SEQ.groove-pool` / `SEQ.groove-library` into rows, so the tree is a pure
//! function of the fields the host already keeps current:
//!
//! - **In use**: pool grooves at least one rack plays;
//! - **Project**: every pool groove;
//! - **Library**: the user's `.groove` files;
//! - **Factory**: the bundled ones (read-only).
//!
//! A pool groove row expands to its instances, one row per rack playing it,
//! labelled `<rack> · T100 V40 R0` (percentages). Every edit goes through the
//! groove host commands the rack panel uses (`rack_grooves.rs`).
//!
//! `(seq-groove-library-heatmap key)` loads one library file's heatmap for
//! the preview below the tree (a pool groove's rides on its
//! `SEQ.groove-pool` entry).

use crate::*;

use sequencer::groove::library::load_library_groove;
use sequencer::groove::GrooveChoice;

fn field(item: &Value, key: &str) -> Value {
    match item {
        Value::Map(map) => map
            .get(key)
            .map(|cell| cell.borrow().clone())
            .unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

fn text(item: &Value, key: &str) -> String {
    match field(item, key) {
        Value::String(value) => value.to_string(),
        _ => String::new(),
    }
}

fn number(item: &Value, key: &str) -> f64 {
    match field(item, key) {
        Value::Number(value) => value,
        _ => 0.0,
    }
}

fn items(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|cell| cell.borrow().clone()).collect(),
        _ => Vec::new(),
    }
}

fn percent(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// `Kit A · T100 V40 R0` (percentages). Compact so the amounts still fit
/// beside a default rack name ("Drum Rack 1") at the narrowest sidebar
/// seq-layout.lisp gives *samples* (34 columns).
pub(crate) fn instance_label(rack: &str, timing: f64, velocity: f64, random: f64) -> String {
    format!(
        "{rack} · T{} V{} R{}",
        percent(timing),
        percent(velocity),
        percent(random)
    )
}

fn header(label: &str, section: &str) -> Value {
    map_value([
        ("label", Value::String(label.to_string().into())),
        ("kind", Value::String("header".into())),
        ("section", Value::String(section.to_string().into())),
    ])
}

fn placeholder(section: &str, label: &str) -> Value {
    map_value([
        ("label", Value::String(label.to_string().into())),
        ("kind", Value::String("empty".into())),
        ("section", Value::String(section.to_string().into())),
        ("path", Value::String(format!("{section}/empty").into())),
        ("draggable", Value::Bool(false)),
        ("drop-target", Value::Bool(false)),
    ])
}

/// One pool groove row under `section` ("in-use" or "project"), with its
/// instances as children. Its `:path` is unique per section, so the same
/// groove listed under In use and Project expands and highlights
/// independently.
fn pool_row(section: &str, groove: &Value, query: &str) -> Option<Value> {
    let id = number(groove, "id") as u64;
    let name = text(groove, "name");
    let key = GrooveChoice::Pool(id).picker_key();
    let path = format!("{section}/{key}");
    let instances = items(&field(groove, "instances"));
    let name_matches = query.is_empty() || name.to_lowercase().contains(query);
    let children: Vec<Value> = instances
        .iter()
        .filter(|instance| name_matches || text(instance, "name").to_lowercase().contains(query))
        .map(|instance| {
            let group_id = number(instance, "group-id") as u64;
            map_value([
                (
                    "label",
                    Value::String(
                        instance_label(
                            &text(instance, "name"),
                            number(instance, "timing"),
                            number(instance, "velocity"),
                            number(instance, "random"),
                        )
                        .into(),
                    ),
                ),
                ("kind", Value::String("instance".into())),
                ("section", Value::String(section.to_string().into())),
                (
                    "path",
                    Value::String(format!("{path}/rack:{group_id}").into()),
                ),
                // No icon and the detail's smaller size, so a default
                // rack name keeps all three amounts at the narrowest
                // sidebar (34 columns).
                ("compact", Value::Bool(true)),
                ("group-id", Value::Number(group_id as f64)),
                ("rack-name", Value::String(text(instance, "name").into())),
                ("groove-id", Value::Number(id as f64)),
                ("groove-name", Value::String(name.clone().into())),
                ("draggable", Value::Bool(false)),
                ("drop-target", Value::Bool(false)),
            ])
        })
        .collect();
    if !name_matches && children.is_empty() {
        return None;
    }
    let rack_names: Vec<Value> = instances
        .iter()
        .map(|instance| Value::String(text(instance, "name").into()))
        .collect();
    let mut fields: Vec<(&'static str, Value)> = vec![
        ("label", Value::String(name.clone().into())),
        ("kind", Value::String("pool".into())),
        ("section", Value::String(section.to_string().into())),
        ("path", Value::String(path.into())),
        ("key", Value::String(key.into())),
        ("groove-id", Value::Number(id as f64)),
        ("name", Value::String(format!("{section}:{id}").into())),
        ("groove-name", Value::String(name.into())),
        ("detail", Value::String(text(groove, "grid").into())),
        ("icon", Value::Keyword("sine".into())),
        ("instance-count", Value::Number(instances.len() as f64)),
        ("rack-names", list_value(rack_names)),
        ("draggable", Value::Bool(false)),
        ("drop-target", Value::Bool(false)),
    ];
    if !instances.is_empty() {
        // Played by at least one rack: the check, then the rack count.
        fields.push(("status-icon", Value::Keyword("check".into())));
        fields.push(("badge", Value::Number(instances.len() as f64)));
        fields.push(("children", list_value(children)));
    }
    Some(map_value(fields))
}

fn library_row(entry: &Value) -> Value {
    let tier = text(entry, "tier");
    let key = text(entry, "key");
    map_value([
        ("label", Value::String(text(entry, "name").into())),
        ("kind", Value::String("library".into())),
        ("section", Value::String(tier.clone().into())),
        ("tier", Value::String(tier.clone().into())),
        ("path", Value::String(format!("library/{key}").into())),
        ("key", Value::String(key.into())),
        ("stem", Value::String(text(entry, "stem").into())),
        ("read-only?", Value::Bool(tier != "user")),
        ("icon", Value::Keyword("document".into())),
        ("draggable", Value::Bool(false)),
        ("drop-target", Value::Bool(false)),
    ])
}

/// The Grooves tab's rows. `pool` is `SEQ.groove-pool`, `library`
/// `SEQ.groove-library`; `query` (already lower-cased) keeps grooves whose
/// name matches, and pool grooves with a matching rack (only those
/// instances). Without a query every section shows, an empty one saying so.
pub(crate) fn groove_tree_value(query: &str, pool: &Value, library: &Value) -> Value {
    let query = query.trim().to_lowercase();
    let searching = !query.is_empty();
    let pool = items(pool);
    let library = items(library);
    let mut rows = Vec::new();
    let mut section = |label: &str, key: &str, body: Vec<Value>, empty: &str| {
        if body.is_empty() && searching {
            return;
        }
        rows.push(header(label, key));
        if body.is_empty() {
            rows.push(placeholder(key, empty));
        } else {
            rows.extend(body);
        }
    };
    let in_use: Vec<Value> = pool
        .iter()
        .filter(|groove| !items(&field(groove, "instances")).is_empty())
        .filter_map(|groove| pool_row("in-use", groove, &query))
        .collect();
    section("In use", "in-use", in_use, "No rack plays a groove");
    let project: Vec<Value> = pool
        .iter()
        .filter_map(|groove| pool_row("project", groove, &query))
        .collect();
    section(
        "Project",
        "project",
        project,
        "Extract a groove from a drum rack, or apply one below",
    );
    let library_rows = |tier: &str| -> Vec<Value> {
        library
            .iter()
            .filter(|entry| text(entry, "tier") == tier)
            .filter(|entry| !searching || text(entry, "name").to_lowercase().contains(&query))
            .map(library_row)
            .collect()
    };
    section(
        "Library",
        "user",
        library_rows("user"),
        "Save a project groove to keep it here",
    );
    section(
        "Factory",
        "factory",
        library_rows("factory"),
        "No factory grooves",
    );
    list_value(rows)
}

pub(crate) fn register_groove_tab_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "seq-groove-tree",
        "(seq-groove-tree query pool library)",
        "Return the Grooves browser tree (In use / Project / Library / Factory) from \
         SEQ.groove-pool and SEQ.groove-library, filtered by query. Pool groove rows carry \
         :groove-id :key :instance-count and one child row per rack instance \
         (:group-id :rack-name); library rows :key :stem :tier :read-only?.",
        |args, _ctx| {
            let query = match args.first() {
                Some(Value::String(query)) => query.to_string(),
                _ => String::new(),
            };
            let pool = args.get(1).cloned().unwrap_or(Value::Nil);
            let library = args.get(2).cloned().unwrap_or(Value::Nil);
            Ok(groove_tree_value(&query, &pool, &library))
        },
    );
    runtime.register_native_with_docs(
        "seq-groove-library-heatmap",
        "(seq-groove-library-heatmap key)",
        "Load library groove `key` (factory:<stem> / user:<stem>) and return its preview \
         heatmap (:slots :grid :rows), or nil when the file cannot be read.",
        |args, _ctx| {
            let Some(Value::String(key)) = args.first() else {
                return Ok(Value::Nil);
            };
            let Ok(GrooveChoice::Library { tier, stem }) = GrooveChoice::from_picker_key(key)
            else {
                return Ok(Value::Nil);
            };
            Ok(load_library_groove(tier, &stem)
                .map(|groove| groove_preview_heatmap(&groove))
                .unwrap_or(Value::Nil))
        },
    );
}

#[cfg(test)]
#[path = "grooves_tab_tests.rs"]
mod tests;
