/*!
Promote an expr card to **My processes** (docs/expr-process-spec.md §8;
bead eseq-waa9.17).

A promoted card becomes one module file of the user-tier package
`user/processes` (`<user_lisp_root>/packages/user.processes/src/<name>.lisp`):

```lisp
(module user.processes.bounce)
(export bounce)

(def-process bounce
  :doc "k shrinks by decay each fire"
  :in ((k :float -1000000 1000000 :default 1 :lane true)
       (decay :float -1000000 1000000 :default 0.8 :lane true))
  :expr "(* k (pow decay $n))")
```

`:expr` hands the body to the same expr pipeline an expr card compiles
through (`expr_process::expr_process_def`), so the promoted class runs
exactly like the card did — inlets, `state` / helper cells, `$` variables,
direct writes, the step budget — and the file stays the player's own
source, not lowered `__expr-*` internals. `:in` only overrides each derived
inlet's range, default and doc (the card's values at promote time become the
defaults). Inside the module the class registers as
`user.processes.<name>/<name>`; the bay and library label it `<name>`.

This file holds the pure parts (name validation, the module text, the
write); the natives live with the other node-patch natives in
`graph_authoring.rs`.
*/

use std::path::{Path, PathBuf};

use super::super::*;

/// Longest name promote accepts.
pub const PROMOTE_NAME_MAX_LEN: usize = 40;

/// The class a promoted process named `name` registers as: `def-process`
/// inside `(module user.processes.<name>)` qualifies its name.
pub fn promoted_class_name(name: &str) -> String {
    format!("{MY_PROCESSES_MODULE_PREFIX}.{name}/{name}")
}

/// The module a promoted process named `name` lives in.
pub fn promoted_module_name(name: &str) -> String {
    format!("{MY_PROCESSES_MODULE_PREFIX}.{name}")
}

/// Where the module file of `name` goes inside the package dir `package_dir`.
pub fn promoted_module_path(package_dir: &Path, name: &str) -> PathBuf {
    package_dir.join("src").join(format!("{name}.lisp"))
}

/// The label a class shows under: its unqualified name.
fn class_base_name(class: &str) -> &str {
    class.rsplit('/').next().unwrap_or(class)
}

/// What a promote under a legal name does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromoteTarget {
    /// A new class and module file.
    New,
    /// Replace the user's own My processes class of that name (its module
    /// file is rewritten and reloaded; every card using it picks it up).
    Update,
}

/// Whether `def` is the user's own My processes class promoted as `name`:
/// the class `user.processes.<name>/<name>` defined by a file inside the My
/// processes package. Only such a class may be updated by a promote.
pub fn is_my_promoted_class(def: &crate::process::PublishedProcessDef, name: &str) -> bool {
    def.name == promoted_class_name(name)
        && def
            .source_path
            .as_deref()
            .is_some_and(super::process_library::is_my_processes_source)
}

/// Check a name for a promoted process against the classes that exist.
/// A name is a lowercase symbol that is also a module segment
/// (`[a-z][a-z0-9-]*`, no trailing or doubled `-`), not `expr`, and not the
/// name or label of any existing class (builtin, package or My processes),
/// so the library and the bay never show two rows with one name — except
/// the user's own My processes class of that name, which the promote then
/// updates ([`PromoteTarget::Update`]). Builtin and other packages' classes
/// are never replaced.
pub fn validate_promote_name(
    name: &str,
    defs: &[crate::process::PublishedProcessDef],
) -> Result<PromoteTarget, String> {
    if name.is_empty() {
        return Err("give the process a name".to_string());
    }
    if name.len() > PROMOTE_NAME_MAX_LEN {
        return Err(format!("a name is at most {PROMOTE_NAME_MAX_LEN} characters"));
    }
    let bytes = name.as_bytes();
    let legal = bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !name.ends_with('-')
        && !name.contains("--");
    if !legal {
        return Err(format!(
            "`{name}` is not a legal name: use lowercase letters, digits and single dashes, starting with a letter"
        ));
    }
    if name == crate::process::EXPR_PROCESS_CLASS || name.starts_with("expr-") {
        return Err(format!("`{name}` is reserved for expr cards"));
    }
    let mut target = PromoteTarget::New;
    for def in defs {
        if crate::process::is_expr_process_class(&def.name) {
            continue;
        }
        let label = super::graph_authoring::graph_node_process_label(&def.name);
        if def.name == name || class_base_name(&def.name) == name || label == name {
            if is_my_promoted_class(def, name) {
                target = PromoteTarget::Update;
                continue;
            }
            return Err(format!("a process named `{name}` already exists"));
        }
    }
    Ok(target)
}

/// A number as the module file writes it: whole values without a
/// fraction, the rest in Rust's shortest round-trip form.
fn number_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else if value.is_finite() {
        format!("{value}")
    } else {
        "0".to_string()
    }
}

/// The class doc for a body: its first line when that is a `;` comment
/// (the semicolons and surrounding space stripped), else a generic line
/// naming the body.
pub fn promoted_doc(source: &str) -> String {
    let first = source.trim_start().lines().next().unwrap_or("").trim();
    if first.starts_with(';') {
        let text = first.trim_start_matches(';').trim();
        if !text.is_empty() {
            // `:doc` is a literal string: no `"` (no escapes).
            return text.replace('"', "'");
        }
    }
    let body: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with(';'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let preview: String = body.chars().take(60).collect();
    let ellipsis = if body.chars().count() > 60 { "…" } else { "" };
    format!("promoted from an expr card: {preview}{ellipsis}").replace('"', "'")
}

/// The native a module file calls for a `"` inside an `:expr` body
/// (eseqlisp strings have no escapes): `(string-from-char-code 34)`.
pub const STRING_FROM_CHAR_CODE_NATIVE: &str = "string-from-char-code";

/// `text` as an eseqlisp expression that evaluates to exactly `text`: a
/// plain string literal, or, when it holds a `"` (strings have no escapes),
/// `(str "…" (string-from-char-code 34) "…")`. `:expr` is evaluated when the
/// module loads, so the body keeps every character, comments included.
fn lisp_string_expression(text: &str) -> String {
    if !text.contains('"') {
        return format!("\"{text}\"");
    }
    let quote = format!("({STRING_FROM_CHAR_CODE_NATIVE} 34)");
    let parts: Vec<String> = text.split('"').map(|part| format!("\"{part}\"")).collect();
    format!("(str {})", parts.join(&format!(" {quote} ")))
}

/// The module text of a promoted process: the header, then one
/// `def-process` with `:doc`, one `:in` entry per inlet (`inlets` = the
/// body's inlets in order with the card's current values, which become the
/// defaults; the range is the expr default until per-slot ranges exist) and
/// the body verbatim under `:expr`. A body with a `"` in it (a string
/// literal, or quotes in a comment) is spelled with
/// [`STRING_FROM_CHAR_CODE_NATIVE`], since eseqlisp strings have no escapes.
pub fn promoted_process_module_source(
    name: &str,
    source: &str,
    inlets: &[(String, f64)],
) -> Result<String, String> {
    let doc = promoted_doc(source);
    let min = number_text(EXPR_INLET_MIN);
    let max = number_text(EXPR_INLET_MAX);
    let inlet_lines: Vec<String> = inlets
        .iter()
        .map(|(inlet, value)| {
            format!(
                "({inlet} :float {min} {max} :default {} :lane true)",
                number_text(*value)
            )
        })
        .collect();
    let in_block = if inlet_lines.is_empty() {
        "()".to_string()
    } else {
        format!("({})", inlet_lines.join("\n       "))
    };
    let module = promoted_module_name(name);
    let expr = lisp_string_expression(source);
    Ok(format!(
        ";; My processes: `{name}`, promoted from an expr card
;; (docs/expr-process-spec.md §8). The class compiles from the :expr body
;; exactly as the card did; :in only sets each inlet's picker range and
;; default. Edit either, then reload the file or restart.
(module {module})
(export {name})

(def-process {name}
  :doc \"{doc}\"
  :in {in_block}
  :expr {expr})
"
    ))
}

/// The `def-process` argument list (after the name) the module file of
/// [`promoted_process_module_source`] evaluates to, for a pre-write check
/// that the class will compile.
pub fn promoted_def_args(source: &str, inlets: &[(String, f64)]) -> Vec<EValue> {
    let keyword = |name: &str| EValue::Keyword(name.to_string());
    let symbol = |name: &str| EValue::Symbol(name.to_string());
    vec![
        keyword("doc"),
        EValue::String(promoted_doc(source)),
        keyword("in"),
        process_list(inlets.iter().map(|(inlet, value)| {
            process_list([
                symbol(inlet),
                symbol("float"),
                EValue::Number(EXPR_INLET_MIN),
                EValue::Number(EXPR_INLET_MAX),
                keyword("default"),
                EValue::Number(*value),
                keyword("lane"),
                EValue::Bool(true),
            ])
        })),
        keyword("expr"),
        EValue::String(source.to_string()),
    ]
}

/// Compile the promoted class in memory, exactly as loading its file will.
pub fn check_promoted_def(
    name: &str,
    source: &str,
    inlets: &[(String, f64)],
) -> Result<crate::process::ProcessDef, String> {
    super::expr_process::expr_process_def_from_args(name, source, &promoted_def_args(source, inlets))
}

const MY_PROCESSES_MANIFEST: &str = "{
  \"name\": \"user/processes\",
  \"version\": \"0.1.0\",
  \"description\": \"My processes: expr cards promoted from the node process bay.\"
}
";

/// Write `contents` as the module file of `name` under `package_dir`,
/// creating the package (manifest.json + src/) on first use. Refuses to
/// replace an existing file unless `replace` (an update of the user's own
/// class, [`PromoteTarget::Update`]). The write goes through a temp file and
/// a rename, so a crash never leaves half a module and an update swaps the
/// file atomically. Returns the file path.
pub fn write_promoted_process(
    package_dir: &Path,
    name: &str,
    contents: &str,
    replace: bool,
) -> Result<PathBuf, String> {
    let src = package_dir.join("src");
    std::fs::create_dir_all(&src)
        .map_err(|error| format!("cannot create {}: {error}", src.display()))?;
    let manifest = package_dir.join("manifest.json");
    if !manifest.exists() {
        std::fs::write(&manifest, MY_PROCESSES_MANIFEST)
            .map_err(|error| format!("cannot write {}: {error}", manifest.display()))?;
    }
    let path = promoted_module_path(package_dir, name);
    if path.exists() && !replace {
        return Err(format!("{} already exists", path.display()));
    }
    let temp = src.join(format!(".{name}.lisp.tmp"));
    std::fs::write(&temp, contents)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot write {}: {error}", path.display())
    })?;
    // The Packages tab and the content tiers see the new package at once.
    crate::app_paths::invalidate_package_catalog_cache();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_named(name: &str) -> crate::process::PublishedProcessDef {
        crate::process::ProcessAuthoringSnapshot {
            defs: vec![crate::process::ProcessDef {
                id: crate::process::stable_process_id(name),
                name: name.to_string(),
                source_path: None,
                expr_source: None,
                doc: None,
                inlets: Vec::new(),
                outlets: Vec::new(),
                state: Vec::new(),
                every: None,
                seed_policy: Default::default(),
                ports: Vec::new(),
                accumulator: None,
                run_source: None,
                listens: Vec::new(),
            }],
            ..Default::default()
        }
        .to_published()
        .unwrap()
        .defs
        .pop()
        .unwrap()
    }

    #[test]
    fn promote_name_validation() {
        let defs = vec![
            def_named("neural-delay"),
            def_named("lane-acc"),
            def_named("user.processes.bounce/bounce"),
            def_named("expr#0123456789ab"),
        ];
        assert_eq!(validate_promote_name("wobble", &defs), Ok(PromoteTarget::New));
        assert_eq!(validate_promote_name("lfsr-2", &defs), Ok(PromoteTarget::New));
        for (bad, why) in [
            ("", "give the process a name"),
            ("Wobble", "not a legal name"),
            ("2x", "not a legal name"),
            ("a b", "not a legal name"),
            ("a/b", "not a legal name"),
            ("../x", "not a legal name"),
            ("a.b", "not a legal name"),
            ("a\\b", "not a legal name"),
            ("é", "not a legal name"),
            ("wob-", "not a legal name"),
            ("wo--b", "not a legal name"),
            ("expr", "reserved"),
            ("expr-2", "reserved"),
            ("neural-delay", "already exists"),
            ("delay", "already exists"),
            ("acc", "already exists"),
            // A class under the My processes id that is NOT defined by a
            // file in the package (no source here) is not the user's own:
            // never replaced.
            ("bounce", "already exists"),
        ] {
            let error = validate_promote_name(bad, &defs).expect_err(bad);
            assert!(error.contains(why), "{bad:?}: {error}");
        }
        assert!(validate_promote_name(&"a".repeat(41), &defs).is_err());
    }

    /// The user's own My processes class (defined by a file in the package)
    /// is updated, not refused; builtin and other packages' classes, or a
    /// label clash beside the own class, still refuse.
    #[test]
    fn promote_name_of_my_own_class_is_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("packages/user.processes");
        let _guard = super::super::process_library::set_my_processes_package_dir_override(Some(package.clone()));
        std::fs::create_dir_all(package.join("src")).unwrap();
        let file = package.join("src/bounce.lisp");
        std::fs::write(&file, "").unwrap();
        let mut mine = def_named("user.processes.bounce/bounce");
        mine.source_path = Some(file.to_string_lossy().into_owned());
        let mut elsewhere = def_named("user.processes.wob/wob");
        elsewhere.source_path = Some(dir.path().join("wob.lisp").to_string_lossy().into_owned());
        let defs = vec![def_named("neural-delay"), mine.clone(), elsewhere];
        assert_eq!(validate_promote_name("bounce", &defs), Ok(PromoteTarget::Update));
        assert!(validate_promote_name("wob", &defs).unwrap_err().contains("already exists"));
        assert!(validate_promote_name("delay", &defs).unwrap_err().contains("already exists"));
        // A builtin labelled like the own class makes the name ambiguous.
        let defs = vec![mine, def_named("lane-bounce")];
        assert!(validate_promote_name("bounce", &defs).unwrap_err().contains("already exists"));
    }

    #[test]
    fn promote_module_text_round_trips_through_def_process() {
        let source = "; k shrinks by decay each fire\n(* k (pow decay $n))";
        let inlets = vec![("k".to_string(), 1.0), ("decay".to_string(), 0.8)];
        let text = promoted_process_module_source("bounce", source, &inlets).unwrap();
        assert!(text.contains("(module user.processes.bounce)\n(export bounce)"), "{text}");
        assert!(text.contains(":doc \"k shrinks by decay each fire\""), "{text}");
        assert!(text.contains("(k :float -1000000 1000000 :default 1 :lane true)"), "{text}");
        assert!(text.contains("(decay :float -1000000 1000000 :default 0.8 :lane true)"), "{text}");
        assert!(text.contains(&format!(":expr \"{source}\")")), "{text}");
        let def = check_promoted_def("bounce", source, &inlets).unwrap();
        assert_eq!(def.doc.as_deref(), Some("k shrinks by decay each fire"));
        assert_eq!(def.expr_source.as_deref(), Some(source));
        assert_eq!(
            def.inlets.iter().map(|i| (i.name.clone(), i.default.clone())).collect::<Vec<_>>(),
            vec![
                ("k".to_string(), EValue::Number(1.0)),
                ("decay".to_string(), EValue::Number(0.8))
            ]
        );
        // No comment: a generic doc naming the body.
        assert_eq!(promoted_doc("(sin in)"), "promoted from an expr card: (sin in)");
        // A `"` (strings have no escapes) is spelled with a char-code call.
        let text = promoted_process_module_source("x", "; say \"hi\"\n(choose \"a\" 1)", &[]).unwrap();
        assert!(
            text.contains(":expr (str \"; say \" (string-from-char-code 34) \"hi\" (string-from-char-code 34) \"\n(choose \" (string-from-char-code 34) \"a\" (string-from-char-code 34) \" 1)\"))"),
            "{text}"
        );
        assert!(text.contains(":doc \"say 'hi'\""), "{text}");
    }

    #[test]
    fn promote_write_creates_the_package_and_replaces_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("packages/user.processes");
        let path = write_promoted_process(&package, "bounce", "(module user.processes.bounce)\n", false).unwrap();
        assert_eq!(path, package.join("src/bounce.lisp"));
        assert!(package.join("manifest.json").is_file());
        let installed = eseqlisp::package::InstalledPackage::load(&package).expect("a valid package");
        assert_eq!(installed.manifest.name, MY_PROCESSES_PACKAGE);
        assert!(write_promoted_process(&package, "bounce", "x", false).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "(module user.processes.bounce)\n");
        // An update replaces it (temp + rename).
        write_promoted_process(&package, "bounce", "(module user.processes.bounce) ;v2\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "(module user.processes.bounce) ;v2\n");
        assert!(!package.join("src/.bounce.lisp.tmp").exists());
        // A dashed name is a legal module segment: the package still validates.
        let text = promoted_process_module_source("my-lfsr-2", "(+ 1 $n)", &[]).unwrap();
        write_promoted_process(&package, "my-lfsr-2", &text, false).unwrap();
        let installed = eseqlisp::package::InstalledPackage::load(&package).expect("a valid package");
        assert_eq!(installed.manifest.name, MY_PROCESSES_PACKAGE);
    }

    /// The promoted class is the card's class under another name: same run
    /// body, state cells (so runtime state carries across the rebind, the
    /// runtime reconciling cells by name), ports and inlet names.
    #[test]
    fn promote_def_lowers_exactly_like_the_card() {
        let source = "(state s 0xACE1) (set! s (bit-xor (shr s 1) (if (= (bit-and s 1) 1) taps 0))) (delay! (* grain (bit-and s 7))) (+ (prev $n) (integ grain) $prev)";
        let card = super::super::expr_process::compile_expr_source(source).unwrap().def.unwrap();
        let inlets = vec![("taps".to_string(), 46080.0), ("grain".to_string(), 1.0)];
        let promoted = crate::process::ProcessAuthoringSnapshot {
            defs: vec![check_promoted_def("lfsr", source, &inlets).unwrap()],
            ..Default::default()
        }
        .to_published()
        .unwrap()
        .defs
        .pop()
        .unwrap();
        assert_eq!(promoted.run_source, card.run_source);
        assert_eq!(promoted.state, card.state);
        assert_eq!(promoted.ports, card.ports);
        assert_eq!(
            promoted.inlets.iter().map(|i| &i.name).collect::<Vec<_>>(),
            card.inlets.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        assert_eq!(promoted.expr_source.as_deref(), Some(source));
        // :in overrides must name a derived inlet.
        let error = check_promoted_def("x", "(* a 2)", &[("b".to_string(), 1.0)]).unwrap_err();
        assert!(error.contains("does not use"), "{error}");
    }
}
