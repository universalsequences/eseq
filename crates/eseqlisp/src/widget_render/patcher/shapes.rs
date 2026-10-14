//! Per-node output shapes and the failed top-level form, from the compiler
//! (docs/patcher-tensor-cables-spec.md §3-§5).
//!
//! The compiler is the only shape authority: it reports each top-level
//! binding's shape (`bindingShapes`) and, when evaluation throws, the failed
//! form (`failedForm`). The patcher emits one top-level form per node, named
//! after the node, so a binding maps back to its node through the emission's
//! rename map ([`PatcherEmissionBindings`], captured with the preview revision
//! that was compiled; an unrenamed node's binding is its id).
//!
//! Results are derived state, stored per patch path outside undo history (like
//! the selected-asset mirror): a compile result replaces the previous one
//! wholesale, and until a new one lands the previous result stays readable, so
//! cables never flicker while a preview compiles. Root view only: shapes inside
//! macro views are not reported in this rev (spec §7).

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::generate::GeneratedPatchSource;
use super::model::{Patch, PatchConnection, PatchNode};

/// What a top-level binding holds. Mirrors the compiler's `BindingShape.Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingShapeKind {
    /// A per-sample scalar signal.
    Signal,
    /// A compile-time number.
    Constant,
    /// A constant (per-patch) tensor.
    Tensor,
    /// A per-sample tensor: a multichannel signal.
    SignalTensor,
    /// A tensor history declared with `@shape`.
    TensorHistory,
    /// Several values; see [`BindingShape::elements`].
    Tuple,
    /// A form with no value (e.g. a write).
    None,
    /// A kind this build does not know (a newer compiler).
    #[serde(other)]
    Unknown,
}

/// One binding's shape as the compiler reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BindingShape {
    pub kind: BindingShapeKind,
    /// The tensor shape; empty for scalars and constants.
    #[serde(default)]
    pub shape: Vec<usize>,
    /// A tuple's element shapes, by position (= outlet index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<BindingShape>>,
}

impl BindingShape {
    /// Element count: the product of the dims, 1 for a scalar.
    pub fn lane_count(&self) -> usize {
        self.shape.iter().product()
    }

    /// Whether a cable carrying this value draws as a multichannel cable
    /// (spec §5): a tensor kind, or any shape with more than one element.
    pub fn is_multichannel(&self) -> bool {
        matches!(
            self.kind,
            BindingShapeKind::Tensor
                | BindingShapeKind::SignalTensor
                | BindingShapeKind::TensorHistory
        ) || self.lane_count() > 1
    }

    /// The shape on outlet `outlet`. A tuple answers by element; anything
    /// else has one output, so only outlet 0 has a shape.
    pub fn outlet(&self, outlet: usize) -> Option<&BindingShape> {
        match (&self.kind, &self.elements) {
            (BindingShapeKind::Tuple, Some(elements)) => elements.get(outlet),
            (BindingShapeKind::Tuple, None) => None,
            _ => (outlet == 0).then_some(self),
        }
    }

    /// `[8 1]`; `[]` for a scalar.
    pub fn dims_label(&self) -> String {
        let dims: Vec<String> = self.shape.iter().map(usize::to_string).collect();
        format!("[{}]", dims.join(" "))
    }

    /// The lane badge text for a multichannel cable: `8`, or `8×1` for
    /// rank ≥ 2. `None` for a scalar.
    pub fn lane_badge(&self) -> Option<String> {
        if !self.is_multichannel() || self.shape.is_empty() {
            return None;
        }
        let dims: Vec<String> = self.shape.iter().map(usize::to_string).collect();
        Some(dims.join("×"))
    }
}

/// The top-level form whose evaluation threw. Mirrors the compiler's
/// `FailedForm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedForm {
    /// Position among the compiled source's top-level forms (including the
    /// host preamble, so not a node index).
    pub index: usize,
    /// The form's operator: `def`, `write-history`, `out`, `param`, …
    pub op: String,
    /// The def's binding / the history a write targets: the node to mark.
    #[serde(default)]
    pub name: Option<String>,
    /// The error, as printed after `Error: `.
    pub message: String,
}

/// The `<dylib-name>.shapes.json` document a compile writes whether or not
/// evaluation succeeds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileShapesReport {
    #[serde(default)]
    pub binding_shapes: HashMap<String, BindingShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_form: Option<FailedForm>,
}

/// Binding name -> root-view node id for one emitted preview revision, plus
/// each root node's text fingerprint at emission time (a failed node's
/// diagnostic stops applying once its text changes). Captured on the preview
/// worker with the source it compiled, so a result always maps back through
/// the emission that produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatcherEmissionBindings {
    binding_to_node: HashMap<String, String>,
    /// Multi-output node id -> its destructured output bindings by outlet;
    /// the compiler reports each of those, never the node itself.
    node_outputs: HashMap<String, Vec<String>>,
    node_text: HashMap<String, u64>,
}

impl PatcherEmissionBindings {
    /// Only root-view entries of `generated` are used. `visible` is the root
    /// patch the source was emitted from.
    pub(super) fn from_emission(visible: &Patch, generated: &GeneratedPatchSource) -> Self {
        let renamed = &generated.renamed_node_ids;
        let mut binding_to_node = HashMap::new();
        let mut node_text = HashMap::new();
        for node in &visible.nodes {
            binding_to_node
                .entry(node.id.clone())
                .or_insert_with(|| node.id.clone());
            node_text.insert(node.id.clone(), node_text_fingerprint(node));
        }
        for ((view_key, node_id), binding) in renamed {
            if view_key != "root" {
                continue;
            }
            // A renamed node's id is not its binding; the binding wins.
            if binding_to_node.get(node_id) == Some(node_id) {
                binding_to_node.remove(node_id);
            }
            binding_to_node.insert(binding.clone(), node_id.clone());
        }
        let node_outputs = generated.root_output_names.clone();
        for (node_id, outputs) in &node_outputs {
            for output in outputs {
                binding_to_node.insert(output.clone(), node_id.clone());
            }
        }
        Self { binding_to_node, node_outputs, node_text }
    }

    /// The node a binding was emitted for.
    pub fn node_for_binding(&self, binding: &str) -> Option<&str> {
        self.binding_to_node.get(binding).map(String::as_str)
    }
}

fn node_text_fingerprint(node: &PatchNode) -> u64 {
    let mut hasher = DefaultHasher::new();
    node.op.hash(&mut hasher);
    format!("{:?}", node.args).hash(&mut hasher);
    hasher.finish()
}

/// A compile failure located at a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedNode {
    pub node_id: String,
    /// The compiler message, prefixed with the form (`at (def b): …`).
    pub message: String,
    pub form: FailedForm,
    /// The node's text fingerprint when the failing revision was emitted.
    text_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct PathCompileShapes {
    /// Root-view node id -> output shape.
    node_shapes: HashMap<String, BindingShape>,
    /// The raw report, for nodes whose id became their binding name after a
    /// save regenerated and reparsed the patch.
    binding_shapes: HashMap<String, BindingShape>,
    failed: Option<FailedNode>,
    /// The form failed but names no node of the root view.
    unlocated_failure: Option<FailedForm>,
}

thread_local! {
    static COMPILE_SHAPES: RefCell<HashMap<String, PathCompileShapes>> =
        RefCell::new(HashMap::new());
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Record a compile result for the patcher editing `path`, replacing the
/// previous one. `bindings` is the emission the compiled source came from
/// ([`super::PreparedPatcherPreview::bindings`]). On success pass the
/// manifest's `bindingShapes` and `failed: None`; on failure pass the
/// shapes.json report's shapes-before-error and its `failedForm`.
pub fn set_compile_shapes_for_path(
    path: impl AsRef<Path>,
    bindings: &PatcherEmissionBindings,
    shapes: HashMap<String, BindingShape>,
    failed: Option<FailedForm>,
) {
    let mut node_shapes: HashMap<String, BindingShape> = shapes
        .iter()
        .filter_map(|(binding, shape)| {
            let node_id = bindings.node_for_binding(binding)?;
            (!bindings.node_outputs.contains_key(node_id))
                .then(|| (node_id.to_string(), shape.clone()))
        })
        .collect();
    // A multi-output node is a destructuring def: reassemble its outlets'
    // shapes into the tuple the node outputs. An outlet the compiler did not
    // reach is unknown, so the tuple is only recorded when all are present.
    for (node_id, outputs) in &bindings.node_outputs {
        let elements: Option<Vec<BindingShape>> =
            outputs.iter().map(|output| shapes.get(output).cloned()).collect();
        if let Some(elements) = elements {
            node_shapes.insert(
                node_id.clone(),
                BindingShape { kind: BindingShapeKind::Tuple, shape: Vec::new(), elements: Some(elements) },
            );
        }
    }
    let mut unlocated_failure = None;
    let failed = failed.and_then(|form| {
        let located = form
            .name
            .as_deref()
            .and_then(|name| bindings.node_for_binding(name))
            .map(str::to_string);
        match located {
            Some(node_id) => Some(FailedNode {
                text_fingerprint: bindings.node_text.get(&node_id).copied(),
                message: failed_form_message(&form),
                node_id,
                form,
            }),
            None => {
                unlocated_failure = Some(form);
                None
            }
        }
    });
    let entry = PathCompileShapes { node_shapes, binding_shapes: shapes, failed, unlocated_failure };
    COMPILE_SHAPES.with(|all| {
        all.borrow_mut().insert(path_key(path.as_ref()), entry);
    });
    crate::widget_render::bump_widget_state_generation();
}

/// Forget the compile result for `path` (session closed).
pub fn clear_compile_shapes_for_path(path: impl AsRef<Path>) {
    let removed = COMPILE_SHAPES.with(|all| all.borrow_mut().remove(&path_key(path.as_ref())));
    if removed.is_some() {
        crate::widget_render::bump_widget_state_generation();
    }
}

fn failed_form_message(form: &FailedForm) -> String {
    match &form.name {
        Some(name) => format!("at ({} {}): {}", form.op, name, form.message),
        None if !form.op.is_empty() => format!("at ({}): {}", form.op, form.message),
        None => form.message.clone(),
    }
}

/// The output shape (outlet 0's value; a tuple for multi-output nodes) of the
/// root-view node `node_id`, from the latest compile result for `path`.
/// `None` = unknown: not compiled yet, the compile failed before reaching the
/// node, the node was added since, or it is inside a macro view.
pub fn node_output_shape(path: impl AsRef<Path>, node_id: &str) -> Option<BindingShape> {
    COMPILE_SHAPES.with(|all| {
        let all = all.borrow();
        let entry = all.get(&path_key(path.as_ref()))?;
        entry
            .node_shapes
            .get(node_id)
            .or_else(|| entry.binding_shapes.get(node_id))
            .cloned()
    })
}

/// The shape a cable from `source_node`'s outlet `outlet` carries: the source
/// node's output shape for outlet 0, a tuple's element by outlet index
/// otherwise. `None` = unknown (draw as a plain cable).
pub fn cable_shape(
    path: impl AsRef<Path>,
    source_node: &str,
    outlet: usize,
) -> Option<BindingShape> {
    node_output_shape(path, source_node)?.outlet(outlet).cloned()
}

/// [`cable_shape`] for a model connection.
pub fn connection_shape(
    path: impl AsRef<Path>,
    connection: &PatchConnection,
) -> Option<BindingShape> {
    cable_shape(path, &connection.from_node, connection.from_output)
}

/// The root-view node the latest compile for `path` failed at, while that
/// node's text is unchanged since the failing revision. `patch` is the
/// current (effective) root patch; pass `None` to skip the edit check.
pub fn failed_compile_node(path: impl AsRef<Path>, patch: Option<&Patch>) -> Option<FailedNode> {
    let failed = COMPILE_SHAPES.with(|all| {
        all.borrow()
            .get(&path_key(path.as_ref()))
            .and_then(|entry| entry.failed.clone())
    })?;
    match patch {
        None => Some(failed),
        Some(patch) => {
            let node = patch.nodes.iter().find(|node| node.id == failed.node_id)?;
            match failed.text_fingerprint {
                Some(fingerprint) if fingerprint != node_text_fingerprint(node) => None,
                _ => Some(failed),
            }
        }
    }
}

/// The latest compile's failed form for `path` when it names no root-view
/// node (a macro-local or preamble form, an `out`).
pub fn unlocated_compile_failure(path: impl AsRef<Path>) -> Option<FailedForm> {
    COMPILE_SHAPES.with(|all| {
        all.borrow()
            .get(&path_key(path.as_ref()))
            .and_then(|entry| entry.unlocated_failure.clone())
    })
}

/// Render pass: mark the failed node of the root view with the compile
/// message (spec §5). An authored diagnostic (unknown operator, …) wins.
pub(super) fn apply_compile_failure_diagnostic(path: &Path, view_key: &str, patch: &mut Patch) {
    if view_key != "root" {
        return;
    }
    let Some(failed) = failed_compile_node(path, Some(patch)) else {
        return;
    };
    if let Some(node) = patch.nodes.iter_mut().find(|node| node.id == failed.node_id)
        && node.diagnostic.is_none()
    {
        node.diagnostic = Some(failed.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget_render::patcher::generate::generate_patch_source;
    use crate::widget_render::patcher::{PatcherIntent, parse_patch_source};

    /// Source ids `created-N` are healed to op-derived binding names on
    /// emission, so these nodes exercise the rename map.
    const SOURCE: &str = "(def gate (in 1 @name gate))\n\
         (def created-1 (phasor 220))\n\
         (def created-0 (* created-1 6.28))\n\
         (def created-2 (cos created-0))\n\
         (out created-2 1 @name audio)\n";

    fn emitted() -> (Patch, PatcherEmissionBindings, HashMap<(String, String), String>) {
        let patch = parse_patch_source(SOURCE, PatcherIntent::Instrument).unwrap();
        let generated = generate_patch_source(&patch, PatcherIntent::Instrument).unwrap();
        let bindings =
            PatcherEmissionBindings::from_emission(&patch, &generated);
        (patch, bindings, generated.renamed_node_ids)
    }

    fn binding_of(renames: &HashMap<(String, String), String>, node_id: &str) -> String {
        renames
            .get(&("root".to_string(), node_id.to_string()))
            .cloned()
            .unwrap_or_else(|| node_id.to_string())
    }

    fn shape(kind: BindingShapeKind, dims: &[usize]) -> BindingShape {
        BindingShape { kind, shape: dims.to_vec(), elements: None }
    }

    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("eseq-patcher-shapes-{name}-{}.lisp", std::process::id()))
    }

    #[test]
    fn multi_output_nodes_take_their_destructured_outputs_as_a_tuple() {
        let source = "(def sig (in 1))\n(def (re im) (fft sig 512 128))\n\
                      (def back (ifft re im 512 128))\n(out back 1)\n";
        let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
        let generated = generate_patch_source(&patch, PatcherIntent::Effect).unwrap();
        let outputs = generated.root_output_names.values().next().cloned().expect("fft outputs");
        let fft_id = generated.root_output_names.keys().next().cloned().unwrap();
        let bindings = PatcherEmissionBindings::from_emission(&patch, &generated);
        let path = test_path("tuple");
        let shapes = HashMap::from([
            (outputs[0].clone(), shape(BindingShapeKind::SignalTensor, &[512])),
            (outputs[1].clone(), shape(BindingShapeKind::SignalTensor, &[512])),
        ]);
        set_compile_shapes_for_path(&path, &bindings, shapes, None);
        for outlet in 0..2 {
            let cable = cable_shape(&path, &fft_id, outlet).expect("fft outlet shape");
            assert!(cable.is_multichannel(), "outlet {outlet} carries the spectrum");
            assert_eq!(cable.shape, vec![512]);
        }
        assert_eq!(cable_shape(&path, &fft_id, 2), None);

        let failed = FailedForm {
            index: 1,
            op: "def".to_string(),
            name: Some(outputs[0].clone()),
            message: "boom".to_string(),
        };
        set_compile_shapes_for_path(&path, &bindings, HashMap::new(), Some(failed));
        assert_eq!(failed_compile_node(&path, None).map(|f| f.node_id), Some(fft_id.clone()));
        assert_eq!(node_output_shape(&path, &fft_id), None, "unreached outlets are unknown");
    }

    #[test]
    fn binding_shapes_map_back_to_nodes_through_the_emission_renames() {
        let (_, bindings, renames) = emitted();
        let renamed = binding_of(&renames, "created-1");
        assert_ne!(renamed, "created-1", "fixture must exercise a rename");
        let path = test_path("renames");
        let shapes = HashMap::from([
            (renamed.clone(), shape(BindingShapeKind::SignalTensor, &[8])),
            ("gate".to_string(), shape(BindingShapeKind::Signal, &[])),
        ]);
        set_compile_shapes_for_path(&path, &bindings, shapes, None);

        let phasor = node_output_shape(&path, "created-1").expect("renamed node maps back");
        assert_eq!(phasor.shape, vec![8]);
        assert!(phasor.is_multichannel());
        assert_eq!(phasor.lane_badge().as_deref(), Some("8"));
        let gate = node_output_shape(&path, "gate").expect("unrenamed node = its id");
        assert!(!gate.is_multichannel());
        assert_eq!(gate.dims_label(), "[]");
        // After a save regenerates and reparses, the node's id is its binding.
        assert_eq!(node_output_shape(&path, &renamed), Some(phasor.clone()));
        // Not in the result: unknown.
        assert_eq!(node_output_shape(&path, "created-0"), None);
        assert_eq!(cable_shape(&path, "created-1", 0), Some(phasor));
        assert_eq!(cable_shape(&path, "created-1", 1), None, "non-tuples have outlet 0 only");
        assert_eq!(node_output_shape(test_path("never-compiled"), "gate"), None);
    }

    #[test]
    fn tuple_outlets_answer_by_element() {
        let tuple = BindingShape {
            kind: BindingShapeKind::Tuple,
            shape: vec![],
            elements: Some(vec![
                shape(BindingShapeKind::Signal, &[]),
                shape(BindingShapeKind::SignalTensor, &[8, 1]),
            ]),
        };
        assert_eq!(tuple.outlet(0).map(|s| s.kind), Some(BindingShapeKind::Signal));
        let second = tuple.outlet(1).unwrap();
        assert_eq!(second.lane_badge().as_deref(), Some("8×1"));
        assert_eq!(second.dims_label(), "[8 1]");
        assert_eq!(tuple.outlet(2), None);
    }

    #[test]
    fn failed_form_marks_its_node_until_edited_or_a_successful_compile() {
        let (patch, bindings, renames) = emitted();
        let path = test_path("failed");
        let form = FailedForm {
            index: 9,
            op: "def".to_string(),
            name: Some(binding_of(&renames, "created-0")),
            message: "Type error: shape mismatch".to_string(),
        };
        let before = HashMap::from([(
            binding_of(&renames, "created-1"),
            shape(BindingShapeKind::SignalTensor, &[8]),
        )]);
        set_compile_shapes_for_path(&path, &bindings, before, Some(form));

        let failed = failed_compile_node(&path, Some(&patch)).expect("failed node located");
        assert_eq!(failed.node_id, "created-0");
        assert!(failed.message.contains("Type error: shape mismatch"), "{}", failed.message);
        // Shapes before the error are kept; the failed node's is unknown.
        assert!(node_output_shape(&path, "created-1").is_some());
        assert_eq!(node_output_shape(&path, "created-0"), None);

        let mut rendered = patch.clone();
        apply_compile_failure_diagnostic(&path, "root", &mut rendered);
        let node = rendered.nodes.iter().find(|node| node.id == "created-0").unwrap();
        assert!(node.diagnostic.as_deref().unwrap().contains("shape mismatch"));
        assert!(
            rendered.nodes.iter().filter(|node| node.diagnostic.is_some()).count() == 1,
            "only the failed node is marked"
        );
        let mut macro_view = patch.clone();
        apply_compile_failure_diagnostic(&path, "macro:foo", &mut macro_view);
        assert!(macro_view.nodes.iter().all(|node| node.diagnostic.is_none()));

        // Editing that node's text drops the derived diagnostic.
        let mut edited = patch.clone();
        let node = edited.nodes.iter_mut().find(|node| node.id == "created-0").unwrap();
        node.op = "+".to_string();
        assert_eq!(failed_compile_node(&path, Some(&edited)), None);
        apply_compile_failure_diagnostic(&path, "root", &mut edited);
        assert!(edited.nodes.iter().all(|node| node.diagnostic.is_none()));

        // A successful compile clears it.
        set_compile_shapes_for_path(&path, &bindings, HashMap::new(), None);
        assert_eq!(failed_compile_node(&path, Some(&patch)), None);
        assert_eq!(failed_compile_node(&path, None), None);
    }

    #[test]
    fn a_failed_form_naming_no_node_is_reported_unlocated() {
        let (patch, bindings, _) = emitted();
        let path = test_path("unlocated");
        let form = FailedForm {
            index: 2,
            op: "out".to_string(),
            name: None,
            message: "bad channel".to_string(),
        };
        set_compile_shapes_for_path(&path, &bindings, HashMap::new(), Some(form.clone()));
        assert_eq!(failed_compile_node(&path, Some(&patch)), None);
        assert_eq!(unlocated_compile_failure(&path), Some(form));
    }

    #[test]
    fn compiler_report_json_deserializes_including_unknown_kinds() {
        let json = r#"{
          "bindingShapes": {
            "a": {"kind": "tensor", "shape": [8]},
            "h": {"kind": "tensor-history", "shape": [4, 2]},
            "t": {"kind": "tuple", "shape": [], "elements": [{"kind": "signal", "shape": []}]},
            "w": {"kind": "none", "shape": []},
            "z": {"kind": "quaternion", "shape": [4]}
          },
          "failedForm": {"index": 1, "message": "boom", "name": "a2", "op": "def"}
        }"#;
        let report: CompileShapesReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.binding_shapes["a"].kind, BindingShapeKind::Tensor);
        assert_eq!(report.binding_shapes["h"].lane_count(), 8);
        assert_eq!(report.binding_shapes["w"].kind, BindingShapeKind::None);
        assert_eq!(report.binding_shapes["z"].kind, BindingShapeKind::Unknown);
        assert_eq!(report.binding_shapes["t"].outlet(0).unwrap().kind, BindingShapeKind::Signal);
        let failed = report.failed_form.unwrap();
        assert_eq!(failed.name.as_deref(), Some("a2"));
        assert_eq!(failed_form_message(&failed), "at (def a2): boom");
        let empty: CompileShapesReport = serde_json::from_str("{}").unwrap();
        assert!(empty.binding_shapes.is_empty() && empty.failed_form.is_none());
    }
}
