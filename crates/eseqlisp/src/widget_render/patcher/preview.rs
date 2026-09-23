use super::*;

/// Immutable editor revision. Capture on the UI thread, then move to the
/// host's compilation worker; preparation never reads live interaction state.
pub struct PatcherPreviewRequest {
    root: Patch,
    state: state::PatcherInteractionState,
    intent: PatcherIntent,
    library: Option<std::sync::Arc<DefmacroLibrary>>,
    metrics: text_metrics::TextMetricsSnapshot,
}

pub struct PreparedPatcherPreview {
    pub source: String,
    pub compile_source: String,
    pub layout: String,
}

impl PatcherPreviewRequest {
    pub fn capture(node: &LayoutNode) -> Result<Self, String> {
        let (_, root) = load_patch_from_props(&node.props)?;
        Ok(Self {
            root,
            state: get_patcher_interaction_state(patcher_state_key(node)),
            intent: patcher_intent_from_props(&node.props),
            library: defmacro_library_for_props(&node.props),
            metrics: text_metrics::TextMetricsSnapshot::capture(),
        })
    }

    pub fn prepare(self) -> Result<PreparedPatcherPreview, String> {
        let Self { root, state, intent, library, metrics } = self;
        metrics.with(|| {
            let root_state = if library.is_some() {
                interaction_state_without_library_macro_views(&state, &root)
            } else {
                state.clone()
            };
            let visible = sidecar::root_patch_with_interaction(&root, &root_state);
            let generated = generate::generate_patch_source(&visible, intent)?;
            let source = generated.source;
            let mut emitted = match library.as_ref() {
                Some(library) => parse_patch_source_with_library(&source, intent, library),
                None => parse_patch_source(&source, intent),
            }.map_err(|error| format!("generated source failed to parse: {error}"))?;
            if !patch_is_fully_projectable(&emitted) {
                return Err(format!(
                    "generated source is not fully projectable: {}",
                    emitted.diagnostics.join("; "),
                ));
            }
            let layout = sidecar::emitted_layout_json_with_node_map(
                &mut emitted, &root, &root_state, &generated.renamed_node_ids,
            )?;
            let compile_source = match library.as_ref() {
                Some(library) => {
                    let staged = library_with_staged_macro_edits(&root, intent, &state, library)
                        .map_err(|error| format!("failed to stage library macro edits: {error}"))?;
                    staged.materialize_source(&source)
                        .map_err(|error| format!("failed to materialize staged defmacro imports: {error}"))?
                        .source
                }
                None => source.clone(),
            };
            Ok(PreparedPatcherPreview { source, compile_source, layout })
        })
    }
}

/// Hosts that own a compilation worker pull the latest revision when this
/// notification is dispatched. Standalone patchers keep the synchronous
/// source payload contract.
pub(super) fn patcher_preview_payload(node: &LayoutNode) -> Value {
    if matches!(node.props.get("deferred-preview"), Some(Value::Bool(true))) {
        let path = prop_str(&node.props, "path").or_else(|| prop_str(&node.props, "file"));
        map_value(vec![
            ("status", Value::Keyword("changed".to_string())),
            ("path", Value::String(path.unwrap_or_default())),
        ])
    } else {
        patcher_writeback_payload(node)
    }
}
