use super::actions::AgentAppAction;
use std::path::{Path, PathBuf};

use super::catalog::{DgenApiCatalog, DocAttribute, DocExample, DocOperator};

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
        let live_examples = self.live_examples_or_catalog();
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
        let live_examples = self.live_examples_or_catalog();
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
        let live_examples = self.live_examples_or_catalog();
        let example = live_examples
            .iter()
            .find(|example| example.name == name)
            .ok_or_else(|| format!("No example named '{name}'."))?;

        let source = std::fs::read_to_string(Path::new(&example.path))
            .map_err(|error| format!("Failed to read '{}': {error}", example.path))?;

        Ok(ToolResult {
            summary: format!("Loaded example '{}' from {}.", example.name, example.path),
            content: source,
            pending_actions: Vec::new(),
        })
    }

    pub fn read_patch_source(&self, kind: ExampleKind, name: &str) -> Result<ToolResult, String> {
        let dir = match kind {
            ExampleKind::Instrument => "instruments",
            ExampleKind::Effect => "effects",
            ExampleKind::Any => {
                return Err("read_patch_source requires an explicit example kind.".to_string())
            }
        };
        let path = Path::new(dir).join(format!("{name}.lisp"));
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read '{}': {error}", path.display()))?;
        Ok(ToolResult {
            summary: format!("Loaded source from {}.", path.display()),
            content: source,
            pending_actions: Vec::new(),
        })
    }

    fn live_examples_or_catalog(&self) -> Vec<DocExample> {
        self.live_examples()
            .unwrap_or_else(|_| self.catalog.examples().to_vec())
    }

    fn live_examples(&self) -> Result<Vec<DocExample>, String> {
        let mut examples = Vec::new();
        for (dir, kind) in [("instruments", "instrument"), ("effects", "effect")] {
            let base = Path::new(dir);
            if !base.exists() {
                continue;
            }

            let mut paths = std::fs::read_dir(base)
                .map_err(|error| format!("Failed to read '{}': {error}", base.display()))?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("lisp"))
                .collect::<Vec<_>>();
            paths.sort();

            for path in paths {
                examples.push(build_live_example(path, kind)?);
            }
        }
        Ok(examples)
    }
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
        name: path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("Invalid example path '{}'.", path.display()))?
            .to_string(),
        kind: kind.to_string(),
        path: path.to_string_lossy().into_owned(),
        params,
        output_count,
        modulator_count,
        preview,
    })
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
    fn list_instrument_examples_returns_known_example() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.list_examples(ExampleKind::Instrument, 50);
        assert!(result.content.contains("prophet-5"));
        assert!(result.content.contains("flute"));
    }

    #[test]
    fn read_example_loads_source() {
        let tools = AgentToolRegistry::load_default().expect("load tools");
        let result = tools.read_example("prophet-5").expect("read example");
        assert!(result.content.contains("(param"));
    }
}
