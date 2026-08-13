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

/// The module every headerless file belongs to (spec §10, slice 0).
pub const IMPLICIT_MODULE: &str = "eseq.vanilla";

/// Blessed always-resolvable namespaces (spec §3 "Core namespaces"):
/// referencing them, bare or qualified, needs no `import`.
pub const CORE_NAMESPACES: &[&str] = &["sdf", "eseq.core"];

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
}
