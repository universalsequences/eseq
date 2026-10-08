//! Module-name concepts for the eseqlisp module system
//! (docs/module-system-spec.md).
//!
//! Slice 0: every headerless file compiles as the implicit module
//! `eseq.vanilla`. Bare global names intern qualified as
//! `eseq.vanilla/name`; resolution falls back to the flat (unqualified)
//! table entry so Rust natives and host-registered globals — which stay
//! flat until namespaced native registration lands (spec §3) — keep
//! resolving. The resolution ladder lives in two places, deliberately in
//! sync: `Compiler::use_global` (compile-time name→index) and
//! `VM::resolve_global_read_index` (runtime by-name lookups).

use crate::parser::{ExprKind, Parser, SpannedASTParser};
use std::collections::{HashMap, HashSet};

/// The module every headerless file belongs to (spec §10, slice 0).
pub const IMPLICIT_MODULE: &str = "eseq.vanilla";

/// Blessed always-resolvable namespaces (spec §3 "Core namespaces"):
/// referencing them, bare or qualified, needs no `import`.
pub const CORE_NAMESPACES: &[&str] = &["sdf", "eseq.core"];

/// Visibility declared by one named module. Named modules are private by
/// default, including modules with no `(export …)` forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleExports {
    names: HashSet<String>,
}

impl ModuleExports {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    pub fn append(&mut self, names: impl IntoIterator<Item = String>) {
        self.names.extend(names);
    }

    pub fn exports(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn names(&self) -> &HashSet<String> {
        &self.names
    }
}

pub type ModuleExportRegistry = HashMap<String, ModuleExports>;

/// Return a visibility decision when `module` is loaded. Implicit/core
/// namespaces are always public; an absent named module has no checkable set.
pub fn exported_from(registry: &ModuleExportRegistry, module: &str, name: &str) -> Option<bool> {
    if module == IMPLICIT_MODULE || CORE_NAMESPACES.contains(&module) {
        return Some(true);
    }
    registry.get(module).map(|exports| exports.exports(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDeclaration {
    pub name: String,
    pub line: usize,
    pub column: usize,
}

/// Read the named module and valid top-level export entries from a source
/// unit. Grammar errors remain the compiler's responsibility; this metadata
/// drives reload replacement and end-of-unit definition validation.
pub fn inspect_exports(
    source: &str,
) -> Result<(Option<String>, Vec<ExportDeclaration>), String> {
    let tokens = Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| format!("parse error: {error:?}"))?;
    let expressions = SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("AST parse error: {error:?}"))?;
    let mut module = None;
    let mut exports = Vec::new();
    for expression in expressions {
        let ExprKind::List(items) = expression.kind else {
            continue;
        };
        match items.as_slice() {
            [head, name] if matches!(&head.kind, ExprKind::Symbol(form) if form == "module") => {
                if let ExprKind::Symbol(name) = &name.kind {
                    module = Some(name.clone());
                }
            }
            [head, names @ ..] if matches!(&head.kind, ExprKind::Symbol(form) if form == "export") =>
            {
                let (line, column) = line_column(source, expression.origin.primary_span.start_byte);
                for name in names {
                    if let ExprKind::Symbol(name) = &name.kind
                        && !name.contains('/')
                    {
                        exports.push(ExportDeclaration {
                            name: name.clone(),
                            line,
                            column,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Ok((module, exports))
}

fn line_column(source: &str, byte: usize) -> (usize, usize) {
    let prefix = &source[..byte.min(source.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len())
        + 1;
    (line, column)
}

/// Split a qualified name at the first `/` into (namespace, base name).
/// Returns None for unqualified names (see `is_qualified`).
pub fn split_qualified(name: &str) -> Option<(&str, &str)> {
    if !is_qualified(name) {
        return None;
    }
    name.split_once('/')
}

/// Valid module name: one or more non-empty dot-separated segments, no
/// `/` anywhere (spec §2: `eseq.mixer`, `sdf`, `alec.acid-tools.riffs`).
pub fn is_valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '*' || c == '%')
        })
}

/// The factory directory holding core modules (`eseq.kinds`), relative to
/// a load-path root.
pub const CORE_MODULES_DIR: &str = "core/modules";

/// Candidate load paths for a module name (spec §7): `eseq.track-collapse`
/// → `track-collapse.lisp` under a load-path root, dots in the remainder
/// mapping to directory separators. `@/` is the source manager cwd — in
/// production that is `crates/sequencer` (`enter_sequencer_dir`), whose
/// vanilla-distro root is its `ui/` subdirectory, so `@/ui/…` candidates
/// are what resolve `eseq.effects.state` → `@/ui/effects/state.lisp`
/// against the real layout. The rootless spellings resolve relative to the
/// importing file (and cover harnesses whose cwd is the ui root itself).
///
/// A module name can also name a directory: `eseq.effects` →
/// `effects/index.lisp` (kind-bindings spec §11), with the precedence
/// documented on `module_relative_file_candidates`.
pub fn module_file_candidates(name: &str) -> Vec<String> {
    let factory = name.starts_with("eseq.");
    let stripped = name.strip_prefix("eseq.").unwrap_or(name);
    let flat = format!("{stripped}.lisp");
    let path = stripped.replace('.', "/");
    let nested = format!("{path}.lisp");
    let index = format!("{path}/index.lisp");
    let prefixes = ["@/ui/", "@/", ""];
    let mut candidates = Vec::new();
    if nested != flat {
        candidates.extend(prefixes.iter().map(|prefix| format!("{prefix}{flat}")));
    }
    for prefix in prefixes {
        candidates.push(format!("{prefix}{index}"));
        candidates.push(format!("{prefix}{nested}"));
    }
    // Factory core modules last (see `module_relative_file_candidates`).
    if factory {
        candidates.push(format!("@/{CORE_MODULES_DIR}/{nested}"));
    }
    candidates
}

/// Paths to try beneath each configured load-path root. Direct spellings map
/// user `modules/` and package `src/` roots; `ui/` spellings map the factory
/// content root. Either shape also supports embedders that choose the more
/// specific root.
///
/// Precedence: a directory's `index.lisp` is tried immediately before its
/// sibling file (`effects/index.lisp` before `effects.lisp`), at each
/// prefix. The sibling file of a directory module is in practice a
/// headerless `load` manifest (ui/effects.lisp loads ui/effects/*), which
/// must never be evaluated as an import. Roots still shadow in order: an
/// earlier root's file beats a later root's index and vice versa. No
/// pre-existing resolution changes, since the index candidates only ever
/// match files named `index.lisp`.
pub fn module_relative_file_candidates(name: &str) -> Vec<std::path::PathBuf> {
    let factory = name.starts_with("eseq.");
    let stripped = name.strip_prefix("eseq.").unwrap_or(name);
    let flat = std::path::PathBuf::from(format!("{stripped}.lisp"));
    let path = stripped.replace('.', "/");
    let nested = std::path::PathBuf::from(format!("{path}.lisp"));
    let index = std::path::PathBuf::from(&path).join("index.lisp");
    let mut candidates = Vec::new();
    for prefix in [std::path::Path::new(""), std::path::Path::new("ui")] {
        if nested != flat {
            candidates.push(prefix.join(&flat));
        }
        candidates.push(prefix.join(&index));
        candidates.push(prefix.join(&nested));
    }
    // Factory core modules (`content/core/modules/kinds.lisp` is
    // `eseq.kinds`, kind-bindings spec §3.4) come last, so a `ui/` module of
    // the same name shadows them. Only `core/modules/` is searched: the
    // startup scripts beside it (`core/init.lisp`, `themes.lisp`, …) are
    // never modules.
    if factory {
        candidates.push(std::path::Path::new(CORE_MODULES_DIR).join(&nested));
    }
    candidates
}

/// True if `name` is already module-qualified (`module/name`). The first
/// `/` splits; a bare `/` (division), a leading `/`, or a trailing `/`
/// does not qualify. Pre-existing flat names that hand-rolled the
/// convention (`sdf/circle`) count as qualified and resolve as-is.
pub fn is_qualified(name: &str) -> bool {
    match name.find('/') {
        Some(idx) => idx > 0 && idx + 1 < name.len(),
        None => false,
    }
}

/// Qualify `name` under `module`.
pub fn qualify(module: &str, name: &str) -> String {
    format!("{module}/{name}")
}

/// Strip the implicit-module prefix for display and host-facing name
/// surfaces (completions, global-store hooks). Identity for flat and
/// explicitly-qualified names.
pub fn strip_implicit(name: &str) -> &str {
    name.strip_prefix(IMPLICIT_MODULE)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_index_precedes_sibling_file() {
        // Single segment: the directory index comes right before its
        // sibling file at each prefix; the old order is otherwise kept.
        assert_eq!(
            module_relative_file_candidates("eseq.effects"),
            [
                "effects/index.lisp",
                "effects.lisp",
                "ui/effects/index.lisp",
                "ui/effects.lisp",
                "core/modules/effects.lisp",
            ]
            .map(std::path::PathBuf::from)
        );
        assert_eq!(
            module_file_candidates("eseq.effects"),
            [
                "@/ui/effects/index.lisp",
                "@/ui/effects.lisp",
                "@/effects/index.lisp",
                "@/effects.lisp",
                "effects/index.lisp",
                "effects.lisp",
                "@/core/modules/effects.lisp",
            ]
        );
        // Dotted: the flat `a.b.lisp` spelling keeps its place ahead of the
        // nested directory forms.
        assert_eq!(
            module_relative_file_candidates("eseq.effects.state"),
            [
                "effects.state.lisp",
                "effects/state/index.lisp",
                "effects/state.lisp",
                "ui/effects.state.lisp",
                "ui/effects/state/index.lisp",
                "ui/effects/state.lisp",
                "core/modules/effects/state.lisp",
            ]
            .map(std::path::PathBuf::from)
        );
        // Only factory (`eseq.`) names reach core/; package modules never do.
        assert!(
            !module_relative_file_candidates("alez.tools")
                .iter()
                .any(|candidate| candidate.starts_with("core"))
        );
    }

    #[test]
    fn startup_scripts_in_core_are_never_module_candidates() {
        // content/core holds init.lisp, themes.lisp and sdf-stdlib.lisp
        // beside the core modules; only core/modules/ is searched.
        for name in ["eseq.init", "eseq.themes", "eseq.sdf-stdlib"] {
            let relative = module_relative_file_candidates(name);
            assert!(
                relative
                    .iter()
                    .all(|candidate| !candidate.starts_with("core")
                        || candidate.starts_with(CORE_MODULES_DIR)),
                "{name}: {relative:?}"
            );
            let rootless = module_file_candidates(name);
            assert!(
                rootless
                    .iter()
                    .all(|candidate| !candidate.contains("core/")
                        || candidate.contains("core/modules/")),
                "{name}: {rootless:?}"
            );
        }
    }

    #[test]
    fn qualification_predicate() {
        assert!(is_qualified("sdf/circle"));
        assert!(is_qualified("eseq.vanilla/foo"));
        assert!(!is_qualified("/"));
        assert!(!is_qualified("foo"));
        assert!(!is_qualified("/leading"));
        assert!(!is_qualified("trailing/"));
        assert!(!is_qualified("*step*"));
    }

    #[test]
    fn module_name_validation() {
        assert!(is_valid_module_name("sdf"));
        assert!(is_valid_module_name("eseq.mixer"));
        assert!(is_valid_module_name("alec.acid-tools.riffs"));
        assert!(!is_valid_module_name(""));
        assert!(!is_valid_module_name("eseq..mixer"));
        assert!(!is_valid_module_name("eseq/mixer"));
        assert!(!is_valid_module_name(".mixer"));
    }

    #[test]
    fn split_qualified_names() {
        assert_eq!(split_qualified("sdf/circle"), Some(("sdf", "circle")));
        assert_eq!(
            split_qualified("eseq.mixer/track-strip"),
            Some(("eseq.mixer", "track-strip"))
        );
        assert_eq!(split_qualified("foo"), None);
        assert_eq!(split_qualified("/"), None);
    }

    #[test]
    fn strip_implicit_prefix() {
        assert_eq!(strip_implicit("eseq.vanilla/foo"), "foo");
        assert_eq!(strip_implicit("foo"), "foo");
        assert_eq!(strip_implicit("sdf/circle"), "sdf/circle");
        assert_eq!(strip_implicit("eseq.vanillaX/foo"), "eseq.vanillaX/foo");
    }

    #[test]
    fn named_modules_export_only_declared_names() {
        let empty = ModuleExports::default();
        assert!(!empty.exports("ordinary"));
        assert!(!empty.exports("%ordinary"));

        let mut exports = ModuleExports::default();
        exports.append(["published".to_string()]);
        exports.append(["also-public".to_string()]);
        assert!(exports.exports("published"));
        assert!(exports.exports("also-public"));
        assert!(!exports.exports("ordinary-private"));
    }

    #[test]
    fn export_inspection_unions_forms_and_records_form_locations() {
        let source = "(module test.exports)\n(export first)\n(def first 1)\n(export second)";
        let (module, exports) = inspect_exports(source).expect("inspect exports");
        assert_eq!(module.as_deref(), Some("test.exports"));
        assert_eq!(
            exports
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!((exports[1].line, exports[1].column), (4, 1));
    }
}
