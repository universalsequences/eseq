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
    /// Binding -> root-view node map of this emission. Hand it back with the
    /// compile result (`set_compile_shapes_for_path`) so shapes map onto the
    /// nodes of the revision that was compiled.
    pub bindings: shapes::PatcherEmissionBindings,
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

    /// The patcher's `:intent`: which compiler entry point the preview uses.
    pub fn intent(&self) -> PatcherIntent {
        self.intent
    }

    pub fn prepare(self) -> Result<PreparedPatcherPreview, String> {
        let Self { root, state, intent, library, metrics } = self;
        metrics.with(|| {
            let (source, layout, bindings) =
                emit_source_and_layout(&root, &state, intent, library.as_deref())?;
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
            Ok(PreparedPatcherPreview { source, compile_source, layout, bindings })
        })
    }

    /// Layout JSON for a layout-only change (node drag, resize, cable
    /// segment). It must key nodes by the same canonical binding names the
    /// writeback payload uses, because the host keeps whichever layout came
    /// last as `last_valid_layout` next to the writeback's source: eject
    /// writes that pair to disk and promotion re-parses the source and matches
    /// sidecar positions by node id. Keying by interaction ids (`created-N`)
    /// here made promotion match none of the editor-created nodes, so every
    /// one of them was auto-laid-out.
    pub(super) fn prepare_layout(self) -> Result<String, String> {
        let Self { root, state, intent, library, metrics } = self;
        metrics.with(|| {
            emit_source_and_layout(&root, &state, intent, library.as_deref())
                .map(|(_, layout, _)| layout)
        })
    }
}

/// Generate the canonical source for the edited patch and the layout sidecar
/// keyed by the node ids a fresh parse of that source assigns (with the
/// serialized graph remapped onto the same ids).
fn emit_source_and_layout(
    root: &Patch,
    state: &state::PatcherInteractionState,
    intent: PatcherIntent,
    library: Option<&DefmacroLibrary>,
) -> Result<(String, String, shapes::PatcherEmissionBindings), String> {
    let root_state = if library.is_some() {
        interaction_state_without_library_macro_views(state, root)
    } else {
        state.clone()
    };
    let visible = sidecar::root_patch_with_interaction(root, &root_state);
    let generated = generate::generate_patch_source(&visible, intent)?;
    let bindings =
        shapes::PatcherEmissionBindings::from_emission(&visible, &generated);
    let source = generated.source;
    let mut emitted = match library {
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
        &mut emitted, root, &root_state, &generated.renamed_node_ids,
    )?;
    Ok((source, layout, bindings))
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
