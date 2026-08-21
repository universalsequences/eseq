use super::actions::AgentAppAction;
use std::path::{Path, PathBuf};

use super::catalog::{DgenApiCatalog, DocAttribute, DocExample, DocOperator, DocSpecialForm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExampleKind {
    Any,
    Instrument,
    Effect,
}

impl ExampleKind {
    fn matches(self, kind: &str) -> bool {
        match self {
            ExampleKind::Any => true,
            ExampleKind::Instrument => kind == "instrument",
            ExampleKind::Effect => kind == "effect",
        }
    }

    pub fn from_wire_value(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "any" => Ok(Self::Any),
            "instrument" => Ok(Self::Instrument),
            "effect" => Ok(Self::Effect),
            _ => Err(format!(
                "Invalid example kind '{}'. Expected any, instrument, or effect.",
                value
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub summary: String,
    pub content: String,
    pub pending_actions: Vec<AgentAppAction>,
}

pub struct AgentToolRegistry {
    catalog: DgenApiCatalog,
}

impl AgentToolRegistry {
    pub fn load_default() -> Result<Self, String> {
        Ok(Self {
            catalog: DgenApiCatalog::load_default()?,
        })
    }

    pub fn new(catalog: DgenApiCatalog) -> Self {
        Self { catalog }
    }

    pub fn catalog(&self) -> &DgenApiCatalog {
        &self.catalog
    }

    pub fn lookup_dgen_docs(&self, queries: &[String], limit: usize) -> ToolResult {
        let limit = limit.max(1);
        let normalized_queries = queries
            .iter()
            .map(|query| query.trim().to_ascii_lowercase())
            .filter(|query| !query.is_empty())
            .collect::<Vec<_>>();
        let joined_queries = normalized_queries.join(", ");
        let effective_queries = if normalized_queries.is_empty() {
            vec![String::new()]
        } else {
            normalized_queries
        };
        let live_examples = self.live_examples().unwrap_or_default();
        let mut sections = Vec::new();

        for query in &effective_queries {
            let mut operators: Vec<&DocOperator> = self
                .catalog
                .operators()
                .iter()
                .filter(|op| {
                    query.is_empty()
                        || op.name.eq_ignore_ascii_case(query)
                        || op
                            .aliases
                            .iter()
                            .any(|alias| alias.eq_ignore_ascii_case(query))
                        || op.name.to_ascii_lowercase().contains(query)
                        || op.summary.to_ascii_lowercase().contains(query)
                        || op
                            .attributes
                            .iter()
                            .any(|attr| attr.to_ascii_lowercase().contains(query))
                })
                .collect();
            operators.sort_by_key(|op| score_operator(op, query));

            let mut special_forms: Vec<&DocSpecialForm> = self
                .catalog
                .special_forms()
                .iter()
                .filter(|form| {
                    query.is_empty()
                        || form.name.eq_ignore_ascii_case(query)
                        || form.name.to_ascii_lowercase().contains(query)
                        || form.summary.to_ascii_lowercase().contains(query)
                })
                .collect();
            special_forms.sort_by_key(|form| score_special_form(form, query));

            let mut attributes: Vec<&DocAttribute> =
                self.catalog
                    .attributes()
                    .iter()
                    .filter(|attr| {
                        query.is_empty()
                            || attr.name.eq_ignore_ascii_case(query)
                            || attr.name.to_ascii_lowercase().contains(query)
                            || attr.summary.to_ascii_lowercase().contains(query)
                            || attr.used_by.iter().any(|name| {
                                name.eq_ignore_ascii_case(query) || name.contains(query)
                            })
                    })
                    .collect();
            attributes.sort_by_key(|attr| score_attribute(attr, query));

            let mut examples: Vec<&DocExample> = live_examples
                .iter()
                .filter(|example| {
                    query.is_empty()
                        || example.name.eq_ignore_ascii_case(query)
                        || example.path.to_ascii_lowercase().contains(query)
                        || example
                            .params
                            .iter()
                            .any(|param| param.to_ascii_lowercase().contains(query))
                })
                .collect();
            examples.sort_by_key(|example| score_example(example, query));

            let mut lines = Vec::new();

            for operator in operators.into_iter().take(limit) {
                let attrs = if operator.attributes.is_empty() {
                    String::new()
                } else {
                    format!(" attrs: {}", operator.attributes.join(", "))
                };
                let signatures = if operator.signatures.is_empty() {
                    String::new()
                } else {
                    format!(" sigs: {}", operator.signatures.join(" | "))
                };
                lines.push(format!(
                    "operator {} [{}] - {}{}{}",
                    operator.name, operator.category, operator.summary, attrs, signatures
                ));
            }

            for form in special_forms.into_iter().take(limit) {
                let signatures = if form.signatures.is_empty() {
                    String::new()
                } else {
                    format!(" sigs: {}", form.signatures.join(" | "))
                };
                lines.push(format!(
                    "special form {} - {}{}",
                    form.name, form.summary, signatures
                ));
            }

            for attribute in attributes.into_iter().take(limit) {
                let used_by = if attribute.used_by.is_empty() {
                    String::new()
                } else {
                    format!(" used_by: {}", attribute.used_by.join(", "))
                };
                lines.push(format!(
                    "attribute {} - {}{}",
                    attribute.name, attribute.summary, used_by
                ));
            }

            for example in examples.into_iter().take(limit) {
                let params = if example.params.is_empty() {
                    String::new()
                } else {
                    format!(" params: {}", example.params.join(", "))
                };
                lines.push(format!(
                    "example {} ({}) path={} outputs={} modulators={}{}",
                    example.name,
                    example.kind,
                    example.path,
                    example.output_count,
                    example.modulator_count,
                    params
                ));
            }

            if let Some(guidance) = effect_authoring_guidance(query) {
                lines.push(guidance.to_string());
            }

            if lines.is_empty() {
                lines.push(format!("No DGenLisp docs matched '{query}'."));
            }

            sections.push(format!("query: {query}\n{}", lines.join("\n")));
        }

        ToolResult {
            summary: format!(
                "Matched docs for {} quer{}{}.",
                effective_queries.len(),
                if effective_queries.len() == 1 {
                    "y"
                } else {
                    "ies"
                },
                if joined_queries.is_empty() {
                    String::new()
                } else {
                    format!(": {}", joined_queries)
                }
            ),
            content: sections.join("\n\n"),
            pending_actions: Vec::new(),
        }
    }

    pub fn list_examples(&self, kind: ExampleKind, limit: usize) -> ToolResult {
        let limit = limit.max(1);
        let live_examples = self.live_examples().unwrap_or_default();
        let examples: Vec<&DocExample> = live_examples
            .iter()
            .filter(|example| kind.matches(&example.kind))
            .take(limit)
            .collect();

        let mut lines = Vec::new();
        for example in examples {
            lines.push(format!(
                "{} ({}) path={} params={} outputs={} modulators={}",
                example.name,
                example.kind,
                example.path,
                example.params.len(),
                example.output_count,
                example.modulator_count
            ));
        }

        if lines.is_empty() {
            lines.push("No examples matched.".to_string());
        }

        ToolResult {
            summary: format!("Listed {} examples.", lines.len()),
            content: lines.join("\n"),
            pending_actions: Vec::new(),
        }
    }

    pub fn read_example(&self, name: &str) -> Result<ToolResult, String> {
        let name = name.trim();
        let live_examples = self.live_examples()?;
        let example = live_examples
            .iter()
            .find(|example| example.name == name)
            .ok_or_else(|| format!("No example named '{name}'."))?;

        let path = Path::new(&example.path);
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("Failed to read '{}': {error}", example.path))?;
        let content = if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
            let ui_path = path.with_file_name("ui.lisp");
            match std::fs::read_to_string(&ui_path) {
                Ok(ui_source) => format!("[dsp.lisp]\n{source}\n\n[ui.lisp]\n{ui_source}"),
                Err(_) => format!("[dsp.lisp]\n{source}"),
            }
        } else {
            source
        };

        Ok(ToolResult {
            summary: format!("Loaded example '{}' from {}.", example.name, example.path),
            content,
            pending_actions: Vec::new(),
        })
    }

    pub fn read_patch_source(&self, kind: ExampleKind, name: &str) -> Result<ToolResult, String> {
        let roots = match kind {
            ExampleKind::Instrument => crate::app_paths::app_paths().instrument_dirs(),
            ExampleKind::Effect => crate::app_paths::app_paths().effect_dirs(),
            ExampleKind::Any => {
                return Err("read_patch_source requires an explicit example kind.".to_string())
            }
        };
        let path = roots
            .iter()
            .flat_map(|root| {
                [
                    root.join(format!("{name}.lisp")),
                    root.join(name).join("dsp.lisp"),
                ]
            })
            .find(|path| path.exists())
            .unwrap_or_else(|| roots[0].join(name).join("dsp.lisp"));
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read '{}': {error}", path.display()))?;
        let content = if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
            let ui_path = path.with_file_name("ui.lisp");
            match std::fs::read_to_string(&ui_path) {
                Ok(ui_source) => format!("[dsp.lisp]\n{source}\n\n[ui.lisp]\n{ui_source}"),
                Err(_) => format!("[dsp.lisp]\n{source}"),
            }
        } else {
            source
        };
        Ok(ToolResult {
            summary: format!("Loaded source from {}.", path.display()),
            content,
            pending_actions: Vec::new(),
        })
    }

    fn live_examples(&self) -> Result<Vec<DocExample>, String> {
        let mut examples = Vec::new();
        let roots = [
            (crate::app_paths::app_paths().instrument_dirs(), "instrument"),
            (crate::app_paths::app_paths().effect_dirs(), "effect"),
        ];
        for (dirs, kind) in roots {
            for base in dirs {
                if !base.exists() {
                    continue;
                }

                let mut paths = collect_patch_source_files(&base)?;
            paths.sort();

                for path in paths {
                    examples.push(build_live_example(path, kind)?);
                }
            }
        }
        Ok(examples)
    }
}

fn effect_authoring_guidance(query: &str) -> Option<&'static str> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.contains("defeffect")
        || normalized.contains("effect boilerplate")
        || normalized.contains("effect wrapper")
    {
        return Some(
            "effect authoring - There is no top-level `defeffect` DSP wrapper. Effect dsp.lisp files are top-level `(def ...)`, `(defmacro ...)`, `(param ...)`, `(in ...)`, and `(out ...)` forms. Use `(defeffect-ui ...)` only in ui.lisp.",
        );
    }

    match normalized.as_str() {
        "group" | "vgroup" | "hgroup" | "knob" | "slider" | "ui" | "ui-control-block"
        | "ui-control-block-header" | "ui-control-block-grid" | "ui-control-block-param"
        | "ui-readout-block" | "ui-readout-block-param" | "@accent" => Some(
            "custom effect UI authoring - Generated ui.lisp should use exactly one `(defeffect-ui <body>)` form with concrete lego helpers. Use `(eseq.effects.custom-ui-lego/ui-control-block-medium-s \"TITLE\" (eseq.effects.custom-ui-lego/ui-accent-blue) section body)`, `(eseq.effects.custom-ui-lego/ui-readout-block-small-s \"TITLE\" (eseq.effects.custom-ui-lego/ui-accent-orange) section body)`, and controls like `(eseq.effects.custom-ui-lego/ui-lego-knob-s section \"param_name\" \"label\" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) decimals)`. Column helpers have fixed arity: `eseq.effects.custom-ui-lego/ui-lego-column` takes three blocks, `eseq.effects.custom-ui-lego/ui-lego-column-2` takes two blocks, and `eseq.effects.custom-ui-lego/ui-lego-column-full` takes one block. Do not put two blocks inside `eseq.effects.custom-ui-lego/ui-lego-column-full`. Do not use legacy or invented wrappers such as `group`, `vgroup`, `hgroup`, `knob`, `slider`, `ui-control-block`, `ui-control-block-header`, `ui-control-block-grid`, or `ui-control-block-param`. Do not use `@accent`; accents are positional helper arguments.",
        ),
        _ => None,
    }
}

fn collect_patch_source_files(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    collect_patch_source_files_into(base, &mut paths)?;
    Ok(paths)
}

fn collect_patch_source_files_into(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let dsp_path = dir.join("dsp.lisp");
    if dsp_path.exists() {
        paths.push(dsp_path);
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)
        .map_err(|error| format!("Failed to read '{}': {error}", dir.display()))?
    {
        let path = entry
            .map_err(|error| format!("Failed to read entry in '{}': {error}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_patch_source_files_into(&path, paths)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("lisp") {
            paths.push(path);
        }
    }
    Ok(())
}

fn build_live_example(path: PathBuf, kind: &str) -> Result<DocExample, String> {
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read '{}': {error}", path.display()))?;
    let params = source
        .lines()
        .filter_map(parse_param_name)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let output_count = source.matches("(out ").count();
    let modulator_count = source.matches("@modulator ").count();
    let preview = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(';') && !line.starts_with('#'))
        .take(6)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(DocExample {
        name: example_name_for_path(&path)?,
        kind: kind.to_string(),
        path: path.to_string_lossy().into_owned(),
        params,
        output_count,
        modulator_count,
        preview,
    })
}

fn example_name_for_path(path: &Path) -> Result<String, String> {
    if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        return path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("Invalid folder-style example path '{}'.", path.display()));
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("Invalid example path '{}'.", path.display()))
}

fn parse_param_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("(param ")?;
    let name = rest.split_whitespace().next()?.trim_end_matches(')');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn score_operator(op: &DocOperator, query: &str) -> (u8, String) {
    if op.name.eq_ignore_ascii_case(query) {
        (0, op.name.clone())
    } else if op
        .aliases
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(query))
    {
        (1, op.name.clone())
    } else if op.name.to_ascii_lowercase().contains(query) {
        (2, op.name.clone())
    } else {
        (3, op.name.clone())
    }
}

fn score_special_form(form: &DocSpecialForm, query: &str) -> (u8, String) {
    if form.name.eq_ignore_ascii_case(query) {
        (0, form.name.clone())
    } else if form.name.to_ascii_lowercase().contains(query) {
        (1, form.name.clone())
    } else {
        (2, form.name.clone())
    }
}

fn score_attribute(attr: &DocAttribute, query: &str) -> (u8, String) {
    if attr.name.eq_ignore_ascii_case(query) {
        (0, attr.name.clone())
    } else if attr.name.to_ascii_lowercase().contains(query) {
        (1, attr.name.clone())
    } else {
        (2, attr.name.clone())
    }
}

fn score_example(example: &DocExample, query: &str) -> (u8, String) {
    if example.name.eq_ignore_ascii_case(query) {
        (0, example.name.clone())
    } else if example.name.to_ascii_lowercase().contains(query) {
        (1, example.name.clone())
    } else {
        (2, example.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentToolRegistry, ExampleKind};

    #[test]
    fn lookup_docs_finds_operator_and_example() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(&["biquad".to_string()], 3);
        assert!(result.content.contains("operator biquad"));
    }

    #[test]
    fn lookup_docs_finds_preamble_filters() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(&["svf".to_string(), "ladder".to_string()], 3);
        assert!(result.content.contains("operator svf"));
        assert!(result.content.contains("operator ladder"));
    }

    #[test]
    fn lookup_docs_finds_mod_and_preamble_envelope_helpers() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(&["mod".to_string(), "adsr".to_string()], 3);
        assert!(result.content.contains("special form mod"));
        assert!(result.content.contains("operator adsr"));
    }

    #[test]
    fn lookup_docs_finds_polyblep_helpers() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(
            &["polyblep_saw".to_string(), "polyblep_pulse".to_string()],
            3,
        );
        assert!(result.content.contains("operator polyblep_saw"));
        assert!(result.content.contains("operator polyblep_pulse"));
    }

    #[test]
    fn lookup_docs_explains_effect_wrapper_shape() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(&["defeffect".to_string()], 3);
        assert!(result.content.contains("no top-level `defeffect`"));
        assert!(result
            .content
            .contains("Use `(defeffect-ui ...)` only in ui.lisp"));
    }

    #[test]
    fn lookup_docs_explains_generated_effect_ui_helpers() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.lookup_dgen_docs(
            &[
                "vgroup".to_string(),
                "knob".to_string(),
                "ui-control-block-param".to_string(),
                "@accent".to_string(),
            ],
            3,
        );
        assert!(result.content.contains("ui-lego-knob-s"));
        assert!(result.content.contains("Do not use legacy"));
        assert!(result.content.contains("Do not use `@accent`"));
        assert!(result.content.contains("ui-lego-column-2"));
    }

    #[test]
    fn list_instrument_examples_returns_known_example() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.list_examples(ExampleKind::Instrument, 200);
        assert!(result.content.contains("prophet-5"));
        assert!(result.content.contains("flute"));
    }

    #[test]
    fn read_example_loads_source() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.read_example("prophet-5").expect("read example");
        assert!(result.content.contains("(param"));
    }

    #[test]
    fn effect_examples_are_named_by_folder_and_include_ui() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let listed = tools.list_examples(ExampleKind::Effect, 200);
        assert!(listed.content.contains("stereo-tremolo (effect)"));
        assert!(!listed.content.contains("ui (effect)"));
        assert!(!listed.content.contains("dsp (effect)"));

        let result = tools
            .read_example("stereo-tremolo")
            .expect("read effect example");
        assert!(result.content.contains("[dsp.lisp]"));
        assert!(result.content.contains("[ui.lisp]"));
        assert!(result.content.contains("(defeffect-ui"));
    }

    #[test]
    fn read_patch_source_supports_folder_style_effects() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools
            .read_patch_source(ExampleKind::Effect, "dualdelaymod")
            .expect("read folder effect");
        assert!(result.content.contains("[dsp.lisp]"));
        assert!(result.content.contains("[ui.lisp]"));
        assert!(result.content.contains("ui-lego-column-2"));
    }
}
