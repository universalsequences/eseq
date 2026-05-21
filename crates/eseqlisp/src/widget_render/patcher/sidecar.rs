use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::display::node_size;
use super::lisp::is_numeric_literal;
use super::model::{CableSegmentInfo, Patch, PatchNode};
use super::state::{
    PatcherInteractionState, patch_with_created_macros, patch_with_interaction_state,
    source_connection_id,
};

const SIDECAR_VERSION: u32 = 1;
const NODE_PADDING: f32 = 1.0;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LayoutSidecar {
    version: u32,
    #[serde(default)]
    root: ScopeLayout,
    #[serde(default)]
    macros: BTreeMap<String, ScopeLayout>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ScopeLayout {
    #[serde(default)]
    nodes: BTreeMap<String, NodePosition>,
    #[serde(default)]
    cables: BTreeMap<String, CableLayout>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct NodePosition {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct CableLayout {
    segmented: bool,
    y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidecarStatus {
    Missing,
    Present,
}

pub(super) fn sidecar_path_for_source(source_path: &Path) -> PathBuf {
    if source_path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        source_path.with_file_name("dsp.layout.json")
    } else {
        let stem = source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dsp");
        source_path.with_file_name(format!("{stem}.layout.json"))
    }
}

pub(super) fn apply_or_materialize(source_path: &Path, patch: &mut Patch) -> Result<(), String> {
    match load_sidecar(source_path) {
        Ok((SidecarStatus::Present, sidecar)) => {
            apply_sidecar(patch, &sidecar);
            save_patch_layout(source_path, patch)
        }
        Ok((SidecarStatus::Missing, _)) => save_patch_layout(source_path, patch),
        Err(error) => {
            eprintln!(
                "failed to load patcher layout sidecar for '{}': {error}",
                source_path.display()
            );
            save_patch_layout(source_path, patch)
        }
    }
}

pub(super) fn save_current_layout(
    source_path: &Path,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), String> {
    let patch = root_patch_with_interaction(root_patch, interaction_state);
    save_patch_layout(source_path, &patch)
}

pub(super) fn current_layout_json(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<String, String> {
    let patch = root_patch_with_interaction(root_patch, interaction_state);
    serde_json::to_string_pretty(&sidecar_from_patch(&patch))
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize layout sidecar: {error}"))
}

pub(super) fn save_emitted_layout(
    source_path: &Path,
    emitted_patch: &mut Patch,
    previous_root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), String> {
    let previous = root_patch_with_interaction(previous_root_patch, interaction_state);
    transfer_positions(&previous, emitted_patch);
    save_patch_layout(source_path, emitted_patch)
}

pub(super) fn emitted_layout_json(
    emitted_patch: &mut Patch,
    previous_root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<String, String> {
    let previous = root_patch_with_interaction(previous_root_patch, interaction_state);
    transfer_positions(&previous, emitted_patch);
    serde_json::to_string_pretty(&sidecar_from_patch(emitted_patch))
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize layout sidecar: {error}"))
}

#[cfg(any(test, feature = "patcher-test-support"))]
pub(super) fn load_apply_for_test(source_path: &Path, patch: &mut Patch) -> Result<(), String> {
    apply_or_materialize(source_path, patch)
}

fn load_sidecar(source_path: &Path) -> Result<(SidecarStatus, LayoutSidecar), String> {
    let path = sidecar_path_for_source(source_path);
    if !path.exists() {
        return Ok((SidecarStatus::Missing, LayoutSidecar::default()));
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let sidecar: LayoutSidecar = serde_json::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    if sidecar.version != SIDECAR_VERSION {
        return Err(format!(
            "unsupported layout sidecar version {} in '{}'",
            sidecar.version,
            path.display()
        ));
    }
    Ok((SidecarStatus::Present, sidecar))
}

fn save_patch_layout(source_path: &Path, patch: &Patch) -> Result<(), String> {
    let sidecar = sidecar_from_patch(patch);
    write_sidecar(source_path, &sidecar)
}

fn write_sidecar(source_path: &Path, sidecar: &LayoutSidecar) -> Result<(), String> {
    let path = sidecar_path_for_source(source_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(sidecar)
        .map_err(|error| format!("failed to serialize layout sidecar: {error}"))?;
    let tmp_path = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("dsp.layout.json")
    ));
    fs::write(&tmp_path, format!("{json}\n"))
        .map_err(|error| format!("failed to write '{}': {error}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        format!("failed to replace '{}': {error}", path.display())
    })
}

fn apply_sidecar(patch: &mut Patch, sidecar: &LayoutSidecar) {
    apply_scope_layout(patch, &sidecar.root);
    for macro_patch in &mut patch.macros {
        if let Some(scope) = sidecar.macros.get(&macro_patch.name) {
            apply_scope_layout(&mut macro_patch.patch, scope);
        }
    }
}

fn apply_scope_layout(patch: &mut Patch, scope: &ScopeLayout) {
    let mut fixed = HashSet::new();
    for node in &mut patch.nodes {
        if let Some(position) = scope
            .nodes
            .get(&node.id)
            .filter(|position| position.x.is_finite() && position.y.is_finite())
        {
            node.position = (position.x, position.y);
            fixed.insert(node.id.clone());
        }
    }
    place_unmatched_nodes(patch, &fixed);

    let connection_endpoint_new = patch
        .connections
        .iter()
        .map(|connection| {
            (
                source_connection_id(connection),
                !fixed.contains(&connection.from_node) || !fixed.contains(&connection.to_node),
            )
        })
        .collect::<HashMap<_, _>>();
    for connection in &mut patch.connections {
        let id = source_connection_id(connection);
        if let Some(cable) = scope.cables.get(&id).filter(|cable| cable.y.is_finite()) {
            connection.segment = cable.segmented.then_some(CableSegmentInfo {
                is_segmented: true,
                segment_row: cable.y,
            });
        } else if !connection_endpoint_new.get(&id).copied().unwrap_or(false) {
            connection.segment = None;
        }
    }
}

fn place_unmatched_nodes(patch: &mut Patch, fixed: &HashSet<String>) {
    let mut occupied = patch
        .nodes
        .iter()
        .filter(|node| fixed.contains(&node.id))
        .map(node_rect)
        .collect::<Vec<_>>();
    for node in &mut patch.nodes {
        if fixed.contains(&node.id) {
            continue;
        }
        let (_, height) = node_size(node);
        while occupied
            .iter()
            .any(|rect| rects_overlap(node_rect(node), *rect))
        {
            node.position.1 += height + NODE_PADDING;
        }
        occupied.push(node_rect(node));
    }
}

fn sidecar_from_patch(patch: &Patch) -> LayoutSidecar {
    let mut macros = BTreeMap::new();
    for macro_patch in &patch.macros {
        macros.insert(
            macro_patch.name.clone(),
            scope_layout_from_patch(&macro_patch.patch),
        );
    }
    LayoutSidecar {
        version: SIDECAR_VERSION,
        root: scope_layout_from_patch(patch),
        macros,
    }
}

fn scope_layout_from_patch(patch: &Patch) -> ScopeLayout {
    let nodes = patch
        .nodes
        .iter()
        .filter(|node| node.position.0.is_finite() && node.position.1.is_finite())
        .map(|node| {
            (
                node.id.clone(),
                NodePosition {
                    x: node.position.0,
                    y: node.position.1,
                },
            )
        })
        .collect();
    let cables = patch
        .connections
        .iter()
        .filter_map(|connection| {
            let segment = connection.segment?;
            (segment.is_segmented && segment.segment_row.is_finite()).then(|| {
                (
                    source_connection_id(connection),
                    CableLayout {
                        segmented: true,
                        y: segment.segment_row,
                    },
                )
            })
        })
        .collect();
    ScopeLayout { nodes, cables }
}

fn root_patch_with_interaction(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Patch {
    let mut root = patch_with_interaction_state(root_patch.clone(), interaction_state, "root");
    let with_created_macros = patch_with_created_macros(root_patch.clone(), interaction_state);
    root.macros = with_created_macros
        .macros
        .into_iter()
        .map(|macro_patch| {
            let view_key = format!("macro:{}", macro_patch.name);
            let mut patch =
                patch_with_interaction_state(macro_patch.patch, interaction_state, &view_key);
            patch.macros = root.macros.clone();
            super::model::MacroPatch {
                name: macro_patch.name,
                params: macro_patch.params,
                patch,
            }
        })
        .collect();
    root
}

fn transfer_positions(previous: &Patch, emitted: &mut Patch) {
    transfer_scope_positions(previous, emitted);
    let previous_macros = previous
        .macros
        .iter()
        .map(|macro_patch| (macro_patch.name.as_str(), &macro_patch.patch))
        .collect::<HashMap<_, _>>();
    for macro_patch in &mut emitted.macros {
        if let Some(previous_patch) = previous_macros.get(macro_patch.name.as_str()) {
            transfer_scope_positions(previous_patch, &mut macro_patch.patch);
        }
    }
}

fn transfer_scope_positions(previous: &Patch, emitted: &mut Patch) {
    let emitted_connections = emitted.connections.clone();
    let previous_by_id = previous
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut used_previous = HashSet::new();
    for node in &mut emitted.nodes {
        if let Some(previous_node) = previous_by_id.get(node.id.as_str()) {
            node.position = previous_node.position;
            used_previous.insert(previous_node.id.as_str());
        }
    }

    let mut previous_by_signature: HashMap<NodeSignature, Vec<&PatchNode>> = HashMap::new();
    for node in &previous.nodes {
        if used_previous.contains(node.id.as_str()) {
            continue;
        }
        previous_by_signature
            .entry(NodeSignature::from_node(node))
            .or_default()
            .push(node);
    }
    for node in &mut emitted.nodes {
        if previous_by_id.contains_key(node.id.as_str()) {
            continue;
        }
        if let Some(previous_node) =
            match_previous_node_by_outputs(previous, &emitted_connections, node, &mut used_previous)
        {
            node.position = previous_node.position;
            continue;
        }
        if let Some(previous_node) =
            match_previous_node_by_inputs(previous, &emitted_connections, node, &mut used_previous)
        {
            node.position = previous_node.position;
            continue;
        }
        if let Some(candidates) = previous_by_signature.get_mut(&NodeSignature::from_node(node))
            && !candidates.is_empty()
        {
            let previous_node = candidates.remove(0);
            node.position = previous_node.position;
        }
    }
}

fn match_previous_node_by_outputs<'a>(
    previous: &'a Patch,
    emitted_connections: &[super::model::PatchConnection],
    emitted_node: &PatchNode,
    used_previous: &mut HashSet<&'a str>,
) -> Option<&'a PatchNode> {
    let emitted_outputs = output_target_signature(emitted_connections, &emitted_node.id);
    if emitted_outputs.is_empty() {
        return None;
    }
    let mut matches = previous
        .nodes
        .iter()
        .filter(|node| !used_previous.contains(node.id.as_str()))
        .filter(|node| output_target_signature(&previous.connections, &node.id) == emitted_outputs)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let node = matches.remove(0);
    used_previous.insert(node.id.as_str());
    Some(node)
}

fn match_previous_node_by_inputs<'a>(
    previous: &'a Patch,
    emitted_connections: &[super::model::PatchConnection],
    emitted_node: &PatchNode,
    used_previous: &mut HashSet<&'a str>,
) -> Option<&'a PatchNode> {
    let emitted_inputs = input_source_signature(emitted_connections, &emitted_node.id);
    if emitted_inputs.is_empty() {
        return None;
    }
    let mut matches = previous
        .nodes
        .iter()
        .filter(|node| !used_previous.contains(node.id.as_str()))
        .filter(|node| node_core_signature(node) == node_core_signature(emitted_node))
        .filter(|node| input_source_signature(&previous.connections, &node.id) == emitted_inputs)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let node = matches.remove(0);
    used_previous.insert(node.id.as_str());
    Some(node)
}

fn node_core_signature(
    node: &PatchNode,
) -> (std::mem::Discriminant<super::model::NodeKind>, &str, usize) {
    (
        std::mem::discriminant(&node.kind),
        node.op.as_str(),
        node.outputs.len(),
    )
}

fn output_target_signature(
    connections: &[super::model::PatchConnection],
    node_id: &str,
) -> Vec<(usize, String, usize)> {
    let mut targets = connections
        .iter()
        .filter(|connection| connection.from_node == node_id)
        .map(|connection| {
            (
                connection.from_output,
                connection.to_node.clone(),
                connection.to_input,
            )
        })
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn input_source_signature(
    connections: &[super::model::PatchConnection],
    node_id: &str,
) -> Vec<(usize, String, usize)> {
    let mut sources = connections
        .iter()
        .filter(|connection| connection.to_node == node_id)
        .map(|connection| {
            (
                connection.to_input,
                connection.from_node.clone(),
                connection.from_output,
            )
        })
        .collect::<Vec<_>>();
    sources.sort();
    sources
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct NodeSignature {
    kind: std::mem::Discriminant<super::model::NodeKind>,
    op: String,
    args: Vec<String>,
    outputs: usize,
}

impl NodeSignature {
    fn from_node(node: &PatchNode) -> Self {
        Self {
            kind: std::mem::discriminant(&node.kind),
            op: node.op.clone(),
            args: node
                .args
                .iter()
                .map(|arg| match arg {
                    super::model::ArgValue::Literal(value) => literal_signature(value),
                    super::model::ArgValue::SymbolRef(_) => "symbol".to_string(),
                    super::model::ArgValue::ConnectedExpr => "connected".to_string(),
                })
                .collect(),
            outputs: node.outputs.len(),
        }
    }
}

fn literal_signature(value: &str) -> String {
    if is_numeric_literal(value) {
        if let Ok(number) = value.parse::<f64>() {
            return format!("number:{number:.9}");
        }
    }
    format!("literal:{value}")
}

fn node_rect(node: &PatchNode) -> (f32, f32, f32, f32) {
    let (width, height) = node_size(node);
    (
        node.position.0,
        node.position.1,
        node.position.0 + width + NODE_PADDING,
        node.position.1 + height + NODE_PADDING,
    )
}

fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

#[cfg(any(test, feature = "patcher-test-support"))]
pub(super) fn save_emitted_layout_for_test(
    source_path: &Path,
    emitted_source: &str,
    intent: super::model::PatcherIntent,
    previous_root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<Patch, String> {
    let mut emitted = super::parse_patch_source(emitted_source, intent)?;
    save_emitted_layout(
        source_path,
        &mut emitted,
        previous_root_patch,
        interaction_state,
    )?;
    Ok(emitted)
}
