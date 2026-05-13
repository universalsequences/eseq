use serde::Deserialize;
use std::path::{Path, PathBuf};

const DEFAULT_DGEN_API_PATH: &str = "docs/dgenlisp-api.json";

#[derive(Debug, Clone, Deserialize)]
pub struct DgenApiDoc {
    pub language: DgenLanguage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DgenLanguage {
    #[serde(default)]
    pub operators: Vec<DocOperator>,
    #[serde(default)]
    pub attributes: Vec<DocAttribute>,
    #[serde(default)]
    pub examples: Vec<DocExample>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocOperator {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub category: String,
    pub summary: String,
    #[serde(default)]
    pub signatures: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocAttribute {
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocExample {
    pub name: String,
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub params: Vec<String>,
    pub output_count: usize,
    pub modulator_count: usize,
    #[serde(default)]
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct DgenApiCatalog {
    pub path: PathBuf,
    pub doc: DgenApiDoc,
}

impl DgenApiCatalog {
    pub fn load_default() -> Result<Self, String> {
        Self::load_from_path(DEFAULT_DGEN_API_PATH)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let src = std::fs::read_to_string(path)
            .map_err(|error| format!("Failed to read '{}': {error}", path.display()))?;
        let doc: DgenApiDoc = serde_json::from_str(&src)
            .map_err(|error| format!("Failed to parse '{}': {error}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            doc,
        })
    }

    pub fn operators(&self) -> &[DocOperator] {
        &self.doc.language.operators
    }

    pub fn attributes(&self) -> &[DocAttribute] {
        &self.doc.language.attributes
    }

    pub fn examples(&self) -> &[DocExample] {
        &self.doc.language.examples
    }
}

#[cfg(test)]
mod tests {
    use super::DgenApiCatalog;

    #[test]
    fn loads_generated_catalog() {
        let catalog = DgenApiCatalog::load_default().expect("load dgen api catalog");
        assert!(!catalog.operators().is_empty());
        assert!(!catalog.attributes().is_empty());
        assert!(!catalog.examples().is_empty());
    }
}
