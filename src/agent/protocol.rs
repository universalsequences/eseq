use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::actions::{
    normalize_patch_name, AgentAppAction, AgentInstrumentPresetDraft, AgentInstrumentPresetSchema,
    AgentSessionContext,
};
use super::tools::{AgentToolRegistry, ExampleKind, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallOutcome {
    pub name: String,
    pub ok: bool,
    pub summary: String,
    pub content: String,
    #[serde(default)]
    pub pending_actions: Vec<AgentAppAction>,
}

pub struct AgentToolRuntime {
    registry: AgentToolRegistry,
}

impl AgentToolRuntime {
    pub fn load_default() -> Result<Self, String> {
        Ok(Self {
            registry: AgentToolRegistry::load_default()?,
        })
    }

    pub fn new(registry: AgentToolRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &AgentToolRegistry {
        &self.registry
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "lookup_dgen_docs".to_string(),
                description: "Look up DGenLisp operators, attributes, and related examples."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Single operator, attribute, topic, or example search term." },
                        "queries": {
                            "type": "array",
                            "description": "List of operators, attributes, topics, or example search terms to look up in one call.",
                            "items": { "type": "string" }
                        },
                        "limit": { "type": "integer", "minimum": 1, "default": 5 }
                    },
                    "anyOf": [
                        { "required": ["query"] },
                        { "required": ["queries"] }
                    ]
                }),
            },
            ToolSpec {
                name: "list_examples".to_string(),
                description:
                    "List available local DGenLisp instrument or effect examples from this repo."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["any", "instrument", "effect"],
                            "default": "any"
                        },
                        "limit": { "type": "integer", "minimum": 1, "default": 20 }
                    }
                }),
            },
            ToolSpec {
                name: "read_example".to_string(),
                description:
                    "Read the full source of a known indexed instrument or effect example."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Indexed example name, e.g. prophet-5." }
                    },
                    "required": ["name"]
                }),
            },
            ToolSpec {
                name: "read_patch_source".to_string(),
                description:
                    "Read a patch source file directly by kind and base name from instruments/ or effects/."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["instrument", "effect"]
                        },
                        "name": { "type": "string", "description": "Patch base name without .lisp suffix." }
                    },
                    "required": ["kind", "name"]
                }),
            },
            ToolSpec {
                name: "create_instrument_track".to_string(),
                description:
                    "Create a new instrument track from generated DGenLisp instrument source."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Short patch name used for saving and the new track label." },
                        "source": { "type": "string", "description": "Complete DGenLisp instrument source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "read_current_instrument_source".to_string(),
                description:
                    "Read the current track's custom instrument source so you can iterate on it instead of rewriting from scratch."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolSpec {
                name: "inspect_current_instrument_preset_schema".to_string(),
                description:
                    "Inspect the current custom instrument's preset schema, editable params, modulation controls, and existing preset names."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolSpec {
                name: "create_current_instrument_presets".to_string(),
                description:
                    "Create one or more named presets for the current custom instrument track using validated runtime parameter names and values."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "presets": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "Preset display name." },
                                    "base_note_offset": { "type": "number", "description": "Optional base note offset in semitones." },
                                    "params": {
                                        "type": "object",
                                        "description": "Map of exact runtime parameter names to numeric values."
                                    }
                                },
                                "required": ["name", "params"]
                            }
                        }
                    },
                    "required": ["presets"]
                }),
            },
            ToolSpec {
                name: "update_current_instrument".to_string(),
                description:
                    "Replace the current custom instrument track's source, save it, and hot-reload it in place."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Instrument name to save under and show on the current track." },
                        "source": { "type": "string", "description": "Complete replacement DGenLisp instrument source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "read_current_effect_source".to_string(),
                description:
                    "Read the currently selected custom effect source so you can iterate on it instead of adding another effect."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolSpec {
                name: "apply_effect_to_current_track".to_string(),
                description:
                    "Apply generated DGenLisp effect source to the current track using the next free custom effect slot."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Short patch name used for saving the effect." },
                        "source": { "type": "string", "description": "Complete DGenLisp effect source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "update_current_effect".to_string(),
                description:
                    "Replace the currently selected custom effect slot's source, save it, and reload it in place."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Effect name to save under and show for the current slot." },
                        "source": { "type": "string", "description": "Complete replacement DGenLisp effect source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
        ]
    }

    pub fn execute(&self, call: ToolCall, session: &AgentSessionContext) -> ToolCallOutcome {
        let result = match call.name.as_str() {
            "lookup_dgen_docs" => self.execute_lookup_docs(&call.arguments),
            "list_examples" => self.execute_list_examples(&call.arguments),
            "read_example" => self.execute_read_example(&call.arguments),
            "read_patch_source" => self.execute_read_patch_source(&call.arguments),
            "create_instrument_track" => self.execute_create_instrument_track(&call.arguments),
            "read_current_instrument_source" => {
                self.execute_read_current_instrument_source(session)
            }
            "inspect_current_instrument_preset_schema" => {
                self.execute_inspect_current_instrument_preset_schema(session)
            }
            "create_current_instrument_presets" => {
                self.execute_create_current_instrument_presets(&call.arguments, session)
            }
            "update_current_instrument" => {
                self.execute_update_current_instrument(&call.arguments, session)
            }
            "read_current_effect_source" => self.execute_read_current_effect_source(session),
            "apply_effect_to_current_track" => {
                self.execute_apply_effect_to_current_track(&call.arguments, session)
            }
            "update_current_effect" => self.execute_update_current_effect(&call.arguments, session),
            _ => Err(format!("Unknown tool '{}'.", call.name)),
        };

        match result {
            Ok(result) => ToolCallOutcome {
                name: call.name,
                ok: true,
                summary: result.summary,
                content: result.content,
                pending_actions: result.pending_actions,
            },
            Err(error) => ToolCallOutcome {
                name: call.name,
                ok: false,
                summary: error.clone(),
                content: error,
                pending_actions: Vec::new(),
            },
        }
    }

    fn execute_lookup_docs(&self, arguments: &Value) -> Result<ToolResult, String> {
        let queries = lookup_queries(arguments)?;
        let limit = optional_usize(arguments, "limit").unwrap_or(5);
        Ok(self.registry.lookup_dgen_docs(&queries, limit))
    }

    fn execute_list_examples(&self, arguments: &Value) -> Result<ToolResult, String> {
        let kind = optional_kind(arguments, "kind")?.unwrap_or(ExampleKind::Any);
        let limit = optional_usize(arguments, "limit").unwrap_or(20);
        Ok(self.registry.list_examples(kind, limit))
    }

    fn execute_read_example(&self, arguments: &Value) -> Result<ToolResult, String> {
        let name = required_string(arguments, "name")?;
        self.registry.read_example(name)
    }

    fn execute_read_patch_source(&self, arguments: &Value) -> Result<ToolResult, String> {
        let kind = optional_kind(arguments, "kind")?
            .ok_or_else(|| "Missing required string field 'kind'.".to_string())?;
        if kind == ExampleKind::Any {
            return Err("Field 'kind' must be 'instrument' or 'effect'.".to_string());
        }
        let name = required_string(arguments, "name")?;
        self.registry.read_patch_source(kind, name)
    }

    fn execute_create_instrument_track(&self, arguments: &Value) -> Result<ToolResult, String> {
        let name =
            normalize_patch_name(required_string(arguments, "name")?, "generated-instrument");
        let source = required_string(arguments, "source")?;
        Ok(ToolResult {
            summary: format!("Queued creation of instrument track '{}'.", name),
            content: format!(
                "Create a new instrument track from generated source '{}'.",
                name
            ),
            pending_actions: vec![AgentAppAction::CreateInstrumentTrack {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_read_current_instrument_source(
        &self,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let name = session.current_instrument_name.as_deref().ok_or_else(|| {
            "No current custom instrument track is selected. Create or select a custom instrument track first."
                .to_string()
        })?;
        let source = session
            .current_instrument_source
            .as_deref()
            .ok_or_else(|| {
                format!(
                    "Current instrument '{}' does not have readable source.",
                    name
                )
            })?;
        Ok(ToolResult {
            summary: format!("Loaded current instrument source for '{}'.", name),
            content: source.to_string(),
            pending_actions: Vec::new(),
        })
    }

    fn execute_inspect_current_instrument_preset_schema(
        &self,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let schema = session.current_instrument_preset_schema.as_ref().ok_or_else(|| {
            "No current custom instrument preset schema is available. Create or select a custom instrument track first."
                .to_string()
        })?;
        let grouped = ["synth", "mod", "source"]
            .into_iter()
            .map(|group| {
                let lines = schema
                    .params
                    .iter()
                    .filter(|param| param.group == group)
                    .map(format_param_schema_line)
                    .collect::<Vec<_>>();
                if lines.is_empty() {
                    None
                } else {
                    Some(format!("group: {group}\n{}", lines.join("\n")))
                }
            })
            .flatten()
            .collect::<Vec<_>>();
        let existing = if schema.existing_presets.is_empty() {
            "none".to_string()
        } else {
            schema.existing_presets.join(", ")
        };

        Ok(ToolResult {
            summary: format!(
                "Inspected preset schema for '{}' ({} params).",
                schema.instrument_name,
                schema.params.len()
            ),
            content: format!(
                "instrument: {}\nsource_file: {}\nbase_note_offset: {}\nexisting_presets: {}\n\n{}",
                schema.instrument_name,
                schema.source_file.as_deref().unwrap_or("<unknown>"),
                schema.base_note_offset,
                existing,
                grouped.join("\n\n")
            ),
            pending_actions: Vec::new(),
        })
    }

    fn execute_create_current_instrument_presets(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let schema = session.current_instrument_preset_schema.as_ref().ok_or_else(|| {
            "No current custom instrument preset schema is available. Create or select a custom instrument track first."
                .to_string()
        })?;
        let presets = parse_preset_drafts(arguments)?;
        if presets.is_empty() {
            return Err("Field 'presets' must contain at least one preset.".to_string());
        }
        for preset in &presets {
            validate_preset_draft_against_schema(preset, schema)?;
        }
        Ok(ToolResult {
            summary: format!(
                "Queued {} preset(s) for '{}'.",
                presets.len(),
                schema.instrument_name
            ),
            content: format!(
                "Save preset bank entries for '{}' with names: {}.",
                schema.instrument_name,
                presets
                    .iter()
                    .map(|preset| preset.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            pending_actions: vec![AgentAppAction::SaveCurrentInstrumentPresets {
                instrument_name: schema.instrument_name.clone(),
                presets,
            }],
        })
    }

    fn execute_update_current_instrument(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.can_update_current_instrument {
            return Err(
                "The current track is not a custom instrument track. Create or select a custom instrument track first."
                    .to_string(),
            );
        }
        let name =
            normalize_patch_name(required_string(arguments, "name")?, "generated-instrument");
        let source = required_string(arguments, "source")?;
        let track = session
            .current_track_name
            .as_deref()
            .unwrap_or("current track");
        Ok(ToolResult {
            summary: format!("Queued instrument update '{}' for {}.", name, track),
            content: format!(
                "Update the current instrument track '{}' using '{}'.",
                track, name
            ),
            pending_actions: vec![AgentAppAction::UpdateCurrentInstrument {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_apply_effect_to_current_track(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.has_tracks {
            return Err(
                "No current track is available. Ask the user to create a track first, then apply the effect."
                    .to_string(),
            );
        }
        if !session.can_apply_effect_to_current_track {
            let track = session
                .current_track_name
                .as_deref()
                .unwrap_or("current track");
            return Err(format!(
                "Track '{}' has no free custom effect slot. Ask the user to free a slot or choose another track.",
                track
            ));
        }
        let name = normalize_patch_name(required_string(arguments, "name")?, "generated-effect");
        let source = required_string(arguments, "source")?;
        let track = session
            .current_track_name
            .as_deref()
            .unwrap_or("current track");
        Ok(ToolResult {
            summary: format!("Queued effect '{}' for {}.", name, track),
            content: format!("Apply generated effect '{}' to {}.", name, track),
            pending_actions: vec![AgentAppAction::ApplyEffectToCurrentTrack {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_read_current_effect_source(
        &self,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let name = session.current_effect_name.as_deref().ok_or_else(|| {
            "No current custom effect slot is selected. Select a custom effect slot first."
                .to_string()
        })?;
        let source = session
            .current_effect_source
            .as_deref()
            .ok_or_else(|| format!("Current effect '{}' does not have readable source.", name))?;
        Ok(ToolResult {
            summary: format!("Loaded current effect source for '{}'.", name),
            content: source.to_string(),
            pending_actions: Vec::new(),
        })
    }

    fn execute_update_current_effect(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.can_update_current_effect {
            return Err(
                "No current custom effect slot is selected. Select a custom effect slot first."
                    .to_string(),
            );
        }
        let name = normalize_patch_name(required_string(arguments, "name")?, "generated-effect");
        let source = required_string(arguments, "source")?;
        let target = session
            .current_effect_name
            .as_deref()
            .unwrap_or("current effect");
        Ok(ToolResult {
            summary: format!("Queued effect update '{}' for {}.", name, target),
            content: format!("Replace current effect '{}' using '{}'.", target, name),
            pending_actions: vec![AgentAppAction::UpdateCurrentEffect {
                name,
                source: source.to_string(),
            }],
        })
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Missing required string field '{key}'."))
}

fn optional_usize(value: &Value, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn optional_kind(value: &Value, key: &str) -> Result<Option<ExampleKind>, String> {
    match value.get(key).and_then(Value::as_str) {
        Some(raw) => ExampleKind::from_wire_value(raw).map(Some),
        None => Ok(None),
    }
}

fn parse_preset_drafts(value: &Value) -> Result<Vec<AgentInstrumentPresetDraft>, String> {
    let items = value
        .get("presets")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'presets'.".to_string())?;
    items.iter().map(parse_preset_draft).collect()
}

fn parse_preset_draft(value: &Value) -> Result<AgentInstrumentPresetDraft, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Each preset requires a non-empty string field 'name'.".to_string())?;
    let params_value = value
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Preset '{name}' requires an object field 'params'."))?;
    let mut params = std::collections::BTreeMap::new();
    for (param_name, raw_value) in params_value {
        let Some(number) = raw_value.as_f64() else {
            return Err(format!(
                "Preset '{name}' param '{}' must be numeric.",
                param_name
            ));
        };
        params.insert(param_name.clone(), number as f32);
    }
    Ok(AgentInstrumentPresetDraft {
        name: name.to_string(),
        base_note_offset: value
            .get("base_note_offset")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        params,
    })
}

fn validate_preset_draft_against_schema(
    preset: &AgentInstrumentPresetDraft,
    schema: &AgentInstrumentPresetSchema,
) -> Result<(), String> {
    if preset.params.is_empty() {
        return Err(format!(
            "Preset '{}' must include at least one parameter value.",
            preset.name
        ));
    }
    for (param_name, value) in &preset.params {
        let param_schema = schema
            .params
            .iter()
            .find(|param| param.name == *param_name)
            .ok_or_else(|| {
                format!(
                    "Preset '{}' references unknown parameter '{}'. Inspect the current instrument preset schema first and use exact parameter names.",
                    preset.name, param_name
                )
            })?;
        if !param_schema.enum_labels.is_empty() {
            let rounded = value.round();
            if (*value - rounded).abs() > 0.0001 {
                return Err(format!(
                    "Preset '{}' param '{}' must be an integer enum index between 0 and {}.",
                    preset.name,
                    param_name,
                    param_schema.enum_labels.len().saturating_sub(1)
                ));
            }
        }
        if *value < param_schema.min || *value > param_schema.max {
            return Err(format!(
                "Preset '{}' param '{}'={} is outside the allowed range [{}, {}].",
                preset.name, param_name, value, param_schema.min, param_schema.max
            ));
        }
    }
    Ok(())
}

fn format_param_schema_line(param: &super::actions::AgentInstrumentParamSchema) -> String {
    let enum_suffix = if param.enum_labels.is_empty() {
        String::new()
    } else {
        format!(" enum=[{}]", param.enum_labels.join(", "))
    };
    let unit_suffix = param
        .unit
        .as_deref()
        .map(|unit| format!(" unit={unit}"))
        .unwrap_or_default();
    let current_suffix = param
        .current_value
        .map(|value| format!(" current={value}"))
        .unwrap_or_default();
    format!(
        "{} range=[{}, {}] default={}{} scaling={}{}{}",
        param.name,
        param.min,
        param.max,
        param.default,
        current_suffix,
        param.scaling,
        unit_suffix,
        enum_suffix
    )
}

fn lookup_queries(value: &Value) -> Result<Vec<String>, String> {
    if let Some(query) = value
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        return Ok(vec![query.to_string()]);
    }

    if let Some(items) = value.get("queries").and_then(Value::as_array) {
        let queries = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !queries.is_empty() {
            return Ok(queries);
        }
    }

    Err(
        "Missing required string field 'query' or non-empty string array field 'queries'."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AgentSessionContext, AgentToolRuntime, ToolCall};
    use crate::agent::actions::{AgentInstrumentParamSchema, AgentInstrumentPresetSchema};

    fn empty_session() -> AgentSessionContext {
        AgentSessionContext::default()
    }

    fn preset_schema_session() -> AgentSessionContext {
        AgentSessionContext {
            has_tracks: true,
            current_track_name: Some("track 1".to_string()),
            current_track_index: Some(0),
            current_instrument_name: Some("prophet-6".to_string()),
            current_instrument_source: Some("(instrument ...)".to_string()),
            can_update_current_instrument: true,
            current_instrument_preset_schema: Some(AgentInstrumentPresetSchema {
                instrument_name: "prophet-6".to_string(),
                source_file: Some("instruments/prophet-6.lisp".to_string()),
                base_note_offset: 0.0,
                existing_presets: vec!["Warm Pad".to_string()],
                params: vec![
                    AgentInstrumentParamSchema {
                        name: "cutoff".to_string(),
                        group: "synth".to_string(),
                        min: 30.0,
                        max: 12000.0,
                        default: 1400.0,
                        current_value: Some(1400.0),
                        unit: Some("Hz".to_string()),
                        enum_labels: Vec::new(),
                        scaling: "exponential".to_string(),
                    },
                    AgentInstrumentParamSchema {
                        name: "mod cutoff src".to_string(),
                        group: "source".to_string(),
                        min: 0.0,
                        max: 6.0,
                        default: 0.0,
                        current_value: Some(0.0),
                        unit: None,
                        enum_labels: vec![
                            "off".to_string(),
                            "env 1".to_string(),
                            "lfo 1".to_string(),
                            "rand".to_string(),
                            "drift".to_string(),
                            "lfo 2".to_string(),
                            "lfo 3".to_string(),
                        ],
                        scaling: "linear".to_string(),
                    },
                ],
            }),
            ..AgentSessionContext::default()
        }
    }

    #[test]
    fn specs_include_lookup_docs() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let names: Vec<String> = runtime.specs().into_iter().map(|spec| spec.name).collect();
        assert!(names.contains(&"lookup_dgen_docs".to_string()));
        assert!(names.contains(&"read_example".to_string()));
        assert!(names.contains(&"inspect_current_instrument_preset_schema".to_string()));
        assert!(names.contains(&"create_current_instrument_presets".to_string()));
    }

    #[test]
    fn execute_lookup_docs_returns_success() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "lookup_dgen_docs".to_string(),
                arguments: json!({ "query": "compressor", "limit": 2 }),
            },
            &empty_session(),
        );
        assert!(outcome.ok);
        assert!(outcome.content.contains("compressor"));
    }

    #[test]
    fn execute_lookup_docs_accepts_multiple_queries() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "lookup_dgen_docs".to_string(),
                arguments: json!({ "queries": ["biquad", "compressor"], "limit": 2 }),
            },
            &empty_session(),
        );
        assert!(outcome.ok);
        assert!(outcome.content.contains("query: biquad"));
        assert!(outcome.content.contains("query: compressor"));
    }

    #[test]
    fn execute_unknown_tool_returns_error() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "not_real".to_string(),
                arguments: json!({}),
            },
            &empty_session(),
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("Unknown tool"));
    }

    #[test]
    fn apply_effect_requires_existing_track() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "apply_effect_to_current_track".to_string(),
                arguments: json!({
                    "name": "wash",
                    "source": "(effect ...)"
                }),
            },
            &empty_session(),
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("create a track first"));
    }

    #[test]
    fn read_current_instrument_source_requires_custom_track() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "read_current_instrument_source".to_string(),
                arguments: json!({}),
            },
            &AgentSessionContext {
                has_tracks: true,
                current_track_name: Some("kick".to_string()),
                current_track_index: Some(0),
                can_apply_effect_to_current_track: true,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
                current_instrument_preset_schema: None,
            },
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("custom instrument track"));
    }

    #[test]
    fn inspect_current_instrument_preset_schema_returns_param_groups() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "inspect_current_instrument_preset_schema".to_string(),
                arguments: json!({}),
            },
            &preset_schema_session(),
        );
        assert!(outcome.ok);
        assert!(outcome.content.contains("instrument: prophet-6"));
        assert!(outcome.content.contains("group: synth"));
        assert!(outcome.content.contains("group: source"));
        assert!(outcome.content.contains("existing_presets: Warm Pad"));
    }

    #[test]
    fn create_current_instrument_presets_validates_and_queues_action() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "create_current_instrument_presets".to_string(),
                arguments: json!({
                    "presets": [{
                        "name": "Glass Sweep",
                        "base_note_offset": 12.0,
                        "params": {
                            "cutoff": 2200.0,
                            "mod cutoff src": 2.0
                        }
                    }]
                }),
            },
            &preset_schema_session(),
        );
        assert!(outcome.ok);
        assert_eq!(outcome.pending_actions.len(), 1);
        assert!(outcome.summary.contains("Queued 1 preset"));
    }

    #[test]
    fn create_current_instrument_presets_rejects_unknown_param_names() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "create_current_instrument_presets".to_string(),
                arguments: json!({
                    "presets": [{
                        "name": "Broken",
                        "params": {
                            "not real": 1.0
                        }
                    }]
                }),
            },
            &preset_schema_session(),
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("unknown parameter"));
    }
}
