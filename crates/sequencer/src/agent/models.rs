//! Runtime model catalog. Parse data without creating a UI VM: background
//! requests and the picker use the same schema and see edits without a rebuild.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use eseqlisp::parser::{Expr, ExprKind, Parser, SpannedASTParser};

use super::providers::{AgentModelPreset, AgentProviderKind, ModelCapability};

#[derive(Debug, Clone)]
pub struct AgentModelCatalog {
    models: Vec<AgentModelPreset>,
}

/// Identity of the file a cached parse came from: views call `agent/models`
/// on every rebuild, so an unchanged file must cost a stat, not a reparse.
/// Any edit changes the mtime (or length), which is what keeps "edit the file,
/// no rebuild needed" working.
type CatalogKey = (PathBuf, Option<SystemTime>, u64);

struct CachedCatalog {
    key: CatalogKey,
    result: Result<AgentModelCatalog, String>,
    /// Whether `load_for_ui` already surfaced `result`'s error.
    error_reported: bool,
}

static CACHE: Mutex<Option<CachedCatalog>> = Mutex::new(None);
static FACTORY_ONLY: AtomicBool = AtomicBool::new(false);

impl AgentModelCatalog {
    /// The active catalog: the user override when present, else the factory
    /// file. Cached per file identity, so repeated calls only stat.
    pub fn load() -> Result<Self, String> {
        Self::load_cached(false).map(|(result, _)| result)?
    }

    /// For views that call this on every rebuild. `Ok(None)` means the
    /// catalog is invalid and that error was already returned once for the
    /// unchanged file, so the caller should not report it again.
    pub fn load_for_ui() -> Result<Option<Self>, String> {
        match Self::load_cached(true)? {
            (Ok(catalog), _) => Ok(Some(catalog)),
            (Err(error), true) => Err(error),
            (Err(_), false) => Ok(None),
        }
    }

    /// Test isolation: read only the factory file, never the developer's
    /// user-root override. Library unit tests get this automatically; other
    /// test targets (the `metal_seq` UI tests) call this first.
    #[doc(hidden)]
    pub fn use_factory_only_for_tests() {
        FACTORY_ONLY.store(true, Ordering::Relaxed);
    }

    /// Returns the (cached) load result and whether its error is new to
    /// `load_for_ui` callers; only those (`mark_reported`) consume that flag.
    fn load_cached(mark_reported: bool) -> Result<(Result<Self, String>, bool), String> {
        let paths = crate::app_paths::app_paths();
        let factory = paths.ui_dir().join("agent-models.lisp");
        let user = if cfg!(test) || FACTORY_ONLY.load(Ordering::Relaxed) {
            None
        } else {
            Some(paths.user_lisp_root().join("agent-models.lisp"))
        };
        Self::load_cached_from(&factory, user.as_deref(), mark_reported)
    }

    fn load_cached_from(
        factory: &Path,
        user: Option<&Path>,
        mark_reported: bool,
    ) -> Result<(Result<Self, String>, bool), String> {
        let key = match user.map(catalog_key) {
            Some(Ok(Some(key))) => key,
            Some(Err(error)) => return Err(error),
            Some(Ok(None)) | None => match catalog_key(factory)? {
                Some(key) => key,
                None => {
                    return Err(format!(
                        "Agent models {}: file not found",
                        factory.display()
                    ))
                }
            },
        };
        let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.as_ref().map_or(true, |cached| cached.key != key) {
            let result = match user {
                Some(user) => Self::load_from_paths(factory, user),
                None => Self::load_from_path(factory),
            };
            *cache = Some(CachedCatalog {
                key,
                result,
                error_reported: false,
            });
        }
        let cached = cache.as_mut().expect("catalog cache filled above");
        let first_error = cached.result.is_err() && !cached.error_reported;
        if cached.result.is_err() && mark_reported {
            cached.error_reported = true;
        }
        Ok((cached.result.clone(), first_error))
    }

    fn load_from_path(path: &Path) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("Agent models {}: {error}", path.display()))?;
        Self::parse(&source).map_err(|error| format!("Agent models {}: {error}", path.display()))
    }

    fn load_from_paths(factory: &Path, user: &Path) -> Result<Self, String> {
        let (path, source) = match std::fs::read_to_string(user) {
            Ok(source) => (user, source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let source = std::fs::read_to_string(factory)
                    .map_err(|error| format!("Agent models {}: {error}", factory.display()))?;
                (factory, source)
            }
            Err(error) => return Err(format!("Agent models {}: {error}", user.display())),
        };
        Self::parse(&source).map_err(|error| format!("Agent models {}: {error}", path.display()))
    }

    pub fn models(&self) -> &[AgentModelPreset] {
        &self.models
    }

    pub fn model(&self, id: &str) -> Option<&AgentModelPreset> {
        self.models.iter().find(|model| model.id == id)
    }

    pub(super) fn parse(source: &str) -> Result<Self, String> {
        let tokens = Parser::new(source.to_string())
            .parse_spanned()
            .map_err(|error| format!("invalid Lisp: {error:?}"))?;
        let forms = SpannedASTParser::new(tokens)
            .parse()
            .map_err(|error| format!("invalid Lisp: {error:?}"))?;
        if forms.len() != 1 {
            return Err("expected one quoted list of model property lists".to_string());
        }
        let ExprKind::QuoteList(rows) = &forms[0].kind else {
            return Err("expected one quoted list of model property lists".to_string());
        };
        if rows.is_empty() {
            return Err("catalog must contain at least one model".to_string());
        }
        let mut models = Vec::with_capacity(rows.len());
        let mut ids = HashSet::new();
        for (index, row) in rows.iter().enumerate() {
            let model =
                parse_model(row).map_err(|error| format!("model {}: {error}", index + 1))?;
            if !ids.insert(model.id.clone()) {
                return Err(format!("duplicate model id {}", model.id));
            }
            models.push(model);
        }
        for provider in AgentProviderKind::ALL {
            let entries: Vec<_> = models
                .iter()
                .filter(|model| model.provider == provider)
                .collect();
            if entries.is_empty() {
                continue;
            }
            if entries.iter().filter(|model| model.default).count() != 1 {
                return Err(format!(
                    "{} needs exactly one :default model",
                    provider.display_name()
                ));
            }
            if entries.iter().filter(|model| model.bubble_default).count() != 1 {
                return Err(format!(
                    "{} needs exactly one :bubble-default model",
                    provider.display_name()
                ));
            }
        }
        Ok(Self { models })
    }
}

/// `Ok(None)` when the file does not exist.
fn catalog_key(path: &Path) -> Result<Option<CatalogKey>, String> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some((path.to_path_buf(), meta.modified().ok(), meta.len()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Agent models {}: {error}", path.display())),
    }
}

fn parse_model(row: &Expr) -> Result<AgentModelPreset, String> {
    let ExprKind::List(items) = &row.kind else {
        return Err("expected a model property list".to_string());
    };
    if items.len() % 2 != 0 {
        return Err("each property needs a value".to_string());
    }
    let mut fields = HashMap::new();
    for pair in items.chunks_exact(2) {
        let ExprKind::Keyword(key) = &pair[0].kind else {
            return Err("property names must be keywords".to_string());
        };
        if !matches!(
            key.as_str(),
            "id" | "name" | "provider" | "capability" | "default" | "bubble-default"
        ) {
            return Err(format!("unknown property :{key}"));
        }
        if fields.insert(key.as_str(), &pair[1].kind).is_some() {
            return Err(format!("duplicate property :{key}"));
        }
    }
    let string = |key| match fields.get(key) {
        Some(ExprKind::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(format!(":{key} must be a nonempty string")),
    };
    let keyword = |key| match fields.get(key) {
        Some(ExprKind::Keyword(value)) => Ok(value.as_str()),
        _ => Err(format!(":{key} must be a keyword")),
    };
    let flag = |key| match fields.get(key) {
        None => Ok(false),
        Some(ExprKind::Symbol(value)) if value == "true" => Ok(true),
        Some(ExprKind::Symbol(value)) if value == "false" => Ok(false),
        _ => Err(format!(":{key} must be true or false")),
    };
    let id = string("id")?;
    if id.chars().any(char::is_whitespace) {
        return Err(":id cannot contain whitespace".to_string());
    }
    let provider = match keyword("provider")? {
        "openai" => AgentProviderKind::OpenAi,
        "gemini" => AgentProviderKind::Gemini,
        "deepseek" => AgentProviderKind::DeepSeek,
        "anthropic" => AgentProviderKind::Anthropic,
        other => return Err(format!("unsupported provider :{other}")),
    };
    let capability = match keyword("capability")? {
        "balanced" => ModelCapability::Balanced,
        "fast" => ModelCapability::Fast,
        "cheap" => ModelCapability::Cheap,
        other => return Err(format!("unsupported capability :{other}")),
    };
    Ok(AgentModelPreset {
        id,
        display_name: string("name")?,
        provider,
        capability,
        default: flag("default")?,
        bubble_default: flag("bubble-default")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "(:id \"test-model\" :name \"Test\" :provider :openai :capability :balanced :default true :bubble-default true)";

    #[test]
    fn catalog_reloads_edits_and_user_override_without_cached_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let factory = dir.path().join("factory.lisp");
        let user = dir.path().join("user.lisp");
        let source = format!("'({MODEL})");
        std::fs::write(&factory, &source).unwrap();
        assert!(AgentModelCatalog::load_from_paths(&factory, &user)
            .unwrap()
            .model("test-model")
            .is_some());
        std::fs::write(&factory, source.replace("test-model", "edited-model")).unwrap();
        assert!(AgentModelCatalog::load_from_paths(&factory, &user)
            .unwrap()
            .model("edited-model")
            .is_some());
        std::fs::write(&user, source.replace("test-model", "user-model")).unwrap();
        let catalog = AgentModelCatalog::load_from_paths(&factory, &user).unwrap();
        assert!(catalog.model("user-model").is_some());
        assert!(catalog.model("edited-model").is_none());
        std::fs::write(&user, "'()").unwrap();
        let error = AgentModelCatalog::load_from_paths(&factory, &user).unwrap_err();
        assert!(error.contains(user.to_str().unwrap()));
        assert!(error.contains("at least one model"));
        std::fs::remove_file(&user).unwrap();
        assert!(AgentModelCatalog::load_from_paths(&factory, &user)
            .unwrap()
            .model("edited-model")
            .is_some());
        std::fs::remove_file(&factory).unwrap();
        assert!(AgentModelCatalog::load_from_paths(&factory, &user).is_err());
    }

    #[test]
    fn cached_catalog_reparses_on_edit_and_reports_each_error_once() {
        let dir = tempfile::tempdir().unwrap();
        let factory = dir.path().join("factory.lisp");
        let user = dir.path().join("user.lisp");
        let source = format!("'({MODEL})");
        std::fs::write(&factory, &source).unwrap();
        let ui = |user: &Path| {
            match AgentModelCatalog::load_cached_from(&factory, Some(user), true).unwrap() {
                (Ok(catalog), _) => Ok(Some(catalog)),
                (Err(error), true) => Err(error),
                (Err(_), false) => Ok(None),
            }
        };
        assert!(ui(&user).unwrap().unwrap().model("test-model").is_some());

        std::fs::write(&user, source.replace(":openai", ":opnai")).unwrap();
        assert!(ui(&user).unwrap_err().contains("unsupported provider"));
        // Unchanged file: cached, and the error is not reported again.
        assert!(ui(&user).unwrap().is_none());
        // Plain `load` callers still see the error.
        assert!(AgentModelCatalog::load_cached_from(&factory, Some(&user), false)
            .unwrap()
            .0
            .is_err());

        // An edit (different length, so the key changes even with a coarse
        // mtime) is reparsed without a restart.
        std::fs::write(&user, source.replace("test-model", "user-model-2")).unwrap();
        assert!(ui(&user).unwrap().unwrap().model("user-model-2").is_some());
        std::fs::remove_file(&user).unwrap();
        assert!(ui(&user).unwrap().unwrap().model("test-model").is_some());
    }

    #[test]
    fn catalog_rejects_ambiguous_or_invalid_data() {
        let valid = format!("'({MODEL})");
        for (source, expected) in [
            (format!("'({MODEL} {MODEL})"), "duplicate model id"),
            (valid.replace(":openai", ":typo"), "unsupported provider"),
            (
                valid.replace(":balanced", ":typo"),
                "unsupported capability",
            ),
            (valid.replace("test-model", "bad id"), "whitespace"),
            (
                valid.replace(":name \"Test\"", ":name \"\""),
                "nonempty string",
            ),
            (valid.replace(":name \"Test\"", ""), "nonempty string"),
            (
                valid.replace(":default true", ":default false"),
                "exactly one :default",
            ),
            (
                valid.replace(":bubble-default true", ""),
                "exactly one :bubble-default",
            ),
            (
                valid.replace(":default true", ":default \"true\""),
                "must be true or false",
            ),
            (
                valid.replace(":default true", ":default true :default true"),
                "duplicate property",
            ),
            (
                valid.replace(":default true", ":defaut true"),
                "unknown property",
            ),
            (format!("{valid} {valid}"), "one quoted list"),
            ("'(broken)".to_string(), "property list"),
            ("'((:id))".to_string(), "needs a value"),
            ("'((id \"x\"))".to_string(), "must be keywords"),
            ("'((".to_string(), "invalid Lisp"),
        ] {
            let error = AgentModelCatalog::parse(&source).unwrap_err();
            assert!(
                error.contains(expected),
                "{source}: {error}, expected {expected}"
            );
        }
        let two_defaults = format!("'({MODEL} {})", MODEL.replace("test-model", "second"));
        assert!(AgentModelCatalog::parse(&two_defaults)
            .unwrap_err()
            .contains("exactly one :default"));
    }

    #[test]
    fn factory_catalog_routes_fable_and_astra() {
        let path = crate::app_paths::app_paths()
            .ui_dir()
            .join("agent-models.lisp");
        let catalog = AgentModelCatalog::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            catalog.model("claude-fable-5-1").unwrap().provider,
            AgentProviderKind::Anthropic
        );
        assert_eq!(
            catalog.model("gpt-6-astra").unwrap().provider,
            AgentProviderKind::OpenAi
        );
    }
}
