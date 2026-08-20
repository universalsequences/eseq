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

/// Visibility declared by one named module. `explicit == false` is the
/// migration-era legacy mode: every name except a `%`-prefixed one is public.
/// The first `(export …)` switches the module to explicit export semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleExports {
    pub explicit: bool,
    names: HashSet<String>,
}

impl ModuleExports {
    pub fn explicit(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            explicit: true,
            names: names.into_iter().collect(),
        }
    }

    pub fn append(&mut self, names: impl IntoIterator<Item = String>) {
        self.explicit = true;
        self.names.extend(names);
    }

    pub fn exports(&self, name: &str) -> bool {
        if self.explicit {
            self.names.contains(name)
        } else {
            !is_private_name(name)
        }
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
) -> Result<(Option<String>, bool, Vec<ExportDeclaration>), String> {
    let tokens = Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| format!("parse error: {error:?}"))?;
    let expressions = SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("AST parse error: {error:?}"))?;
    let mut module = None;
    let mut has_export_form = false;
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
                has_export_form = true;
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
    Ok((module, has_export_form, exports))
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

/// True if a base name is marked internal by the `%` privacy convention
/// (spec §2 decision 4).
pub fn is_private_name(base: &str) -> bool {
    base.starts_with('%')
}

/// Candidate load paths for a module name (spec §7): `eseq.track-collapse`
/// → `track-collapse.lisp` under a load-path root, dots in the remainder
/// mapping to directory separators. `@/` is the source manager cwd — in
/// production that is `crates/sequencer` (`enter_sequencer_dir`), whose
/// vanilla-distro root is its `ui/` subdirectory, so `@/ui/…` candidates
/// are what resolve `eseq.effects.state` → `@/ui/effects/state.lisp`
/// against the real layout. The rootless spellings resolve relative to the
/// importing file (and cover harnesses whose cwd is the ui root itself).
pub fn module_file_candidates(name: &str) -> Vec<String> {
    let stripped = name.strip_prefix("eseq.").unwrap_or(name);
    let flat = format!("{stripped}.lisp");
    let nested = format!("{}.lisp", stripped.replace('.', "/"));
    let mut candidates = vec![
        format!("@/ui/{flat}"),
        format!("@/{flat}"),
        flat.clone(),
    ];
    if nested != flat {
        candidates.push(format!("@/ui/{nested}"));
        candidates.push(format!("@/{nested}"));
        candidates.push(nested);
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
    fn explicit_exports_replace_legacy_percent_visibility() {
        let legacy = ModuleExports::default();
        assert!(legacy.exports("public"));
        assert!(!legacy.exports("%private"));

        let mut explicit = ModuleExports::default();
        explicit.append(["%published".to_string()]);
        explicit.append(["also-public".to_string()]);
        assert!(explicit.exports("%published"));
        assert!(explicit.exports("also-public"));
        assert!(!explicit.exports("ordinary-private"));

        let mut empty = ModuleExports::default();
        empty.append(Vec::<String>::new());
        assert!(empty.explicit);
        assert!(!empty.exports("formerly-public"));
    }

    #[test]
    fn export_inspection_unions_forms_and_records_form_locations() {
        let source = "(module test.exports)\n(export first)\n(def first 1)\n(export second)";
        let (module, explicit, exports) = inspect_exports(source).expect("inspect exports");
        assert_eq!(module.as_deref(), Some("test.exports"));
        assert!(explicit);
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
