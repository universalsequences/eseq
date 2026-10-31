use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eseqlisp::Editor;
use sequencer::custom_ui_codegen::{
    custom_ui_param_name, expr_to_lisp, instrument_ui_definitions, local_helper_names,
    namespace_local_helpers, safe_lisp_ident, transform_layout_items_without_unbounded_width,
};
pub(crate) use sequencer::custom_ui_codegen::lisp_string_literal;

const GENERATED_INSTRUMENT_UI_PATH: &str = ".metal-seq-generated/custom-instrument-ui.lisp";
const GENERATED_MIDI_FX_UI_PATH: &str = ".metal-seq-generated/custom-midi-fx-ui.lisp";
const GENERATED_AUDIO_FX_UI_PATH: &str = ".metal-seq-generated/custom-audio-fx-ui.lisp";

/// The panels of draft instruments outside the instrument roots (modal
/// script drafts): engine name -> (ui.lisp path, for messages; its source).
/// Every instrument UI rebuild includes them, so a hot reload of another
/// panel keeps the drafts'. A static because the rebuild also runs on a
/// worker and from callers that only hold the editor.
pub(crate) type DraftInstrumentUis = BTreeMap<String, (String, String)>;
static DRAFT_INSTRUMENT_UIS: std::sync::Mutex<DraftInstrumentUis> =
    std::sync::Mutex::new(BTreeMap::new());

/// Replace every draft panel; takes effect on the next instrument UI rebuild.
pub(crate) fn set_draft_instrument_uis(uis: DraftInstrumentUis) {
    *DRAFT_INSTRUMENT_UIS.lock().unwrap_or_else(|poison| poison.into_inner()) = uis;
}


fn transform_midi_fx_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if head == "defwidget" {
                    return expr_to_lisp(expr);
                }
                if head == "midi-fx-param" {
                    if let Some(name) = items.get(1).and_then(custom_ui_param_name) {
                        return format!(
                            "(eseq.effects.custom-effect-ui/midi-fx-ui-param-control {})",
                            lisp_string_literal(&name)
                        );
                    }
                }
                if head == "params" {
                    let mut controls = Vec::new();
                    for item in items.iter().skip(1) {
                        if matches!(item, Expression::Keyword(_)) {
                            continue;
                        }
                        if let Some(name) = custom_ui_param_name(item) {
                            controls.push(format!(
                                "(eseq.effects.custom-effect-ui/midi-fx-ui-param-control {})",
                                lisp_string_literal(&name)
                            ));
                        }
                    }
                    return format!("(v-stack :gap 0.25 {})", controls.join(" "));
                }
            }
            format!(
                "({})",
                transform_layout_items_without_unbounded_width(items, transform_midi_fx_ui_expr)
                    .join(" ")
            )
        }
        _ => expr_to_lisp(expr),
    }
}

fn transform_audio_fx_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if head == "defwidget" {
                    return expr_to_lisp(expr);
                }
                if matches!(head.as_str(), "effect-param" | "fx-param" | "param") {
                    if let Some(name) = items.get(1).and_then(custom_ui_param_name) {
                        return format!(
                            "(eseq.effects.custom-effect-ui/audio-fx-ui-param-control {})",
                            lisp_string_literal(&name)
                        );
                    }
                }
                if head == "params" {
                    let mut controls = Vec::new();
                    for item in items.iter().skip(1) {
                        if matches!(item, Expression::Keyword(_)) {
                            continue;
                        }
                        if let Some(name) = custom_ui_param_name(item) {
                            controls.push(format!(
                                "(eseq.effects.custom-effect-ui/audio-fx-ui-param-control {})",
                                lisp_string_literal(&name)
                            ));
                        }
                    }
                    return format!("(v-stack :gap 0.25 {})", controls.join(" "));
                }
            }
            format!(
                "({})",
                transform_layout_items_without_unbounded_width(items, transform_audio_fx_ui_expr)
                    .join(" ")
            )
        }
        _ => expr_to_lisp(expr),
    }
}

/// The generated function rendering one MIDI/audio effect's custom UI
/// (`kind` "midi-fx" or "audio-fx"). The body, and the read of the selected
/// section it highlights, live in a keyed subtree of their own: a section
/// change (every knob edit in an unselected section selects it) re-runs the
/// body alone, not the effect panel around it, which rebuilds the effect's
/// whole panel dict (every param). The subtree re-sets the ambient render
/// globals, which belong to whichever panel rendered last when it re-runs
/// alone.
fn effect_ui_function_source(fn_name: &str, kind: &str, name_literal: &str, body: &str) -> String {
    let globals = format!(
        "(set! {kind}-ui-current-fx fx) (set! {kind}-ui-current-name {name_literal}) (set! custom-ui-current-kind \"{kind}\")"
    );
    format!(
        "\n(def {fn_name} (fx) (do {globals} (subtree :key (str \"custom-ui-body-\" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) \"-\" (get fx :track-idx) \"-\" (get fx :rack-slot)) (do {globals} (set! custom-ui-selected-section (eseq.effects.custom-ui-sections/custom-ui-selected-section-for-current-scope)) {body}))))\n"
    )
}


fn instrument_logical_name(instrument_name: &str) -> &str {
    match sequencer::app_paths::ContentTier::parse_id(instrument_name) {
        Ok(Some((_, logical))) => logical,
        _ => instrument_name.trim_end_matches('/'),
    }
}

fn instrument_leaf_name(instrument_name: &str) -> Option<String> {
    let normalized = instrument_logical_name(instrument_name);
    Path::new(normalized)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn instrument_ui_dispatch_aliases(
    instrument_name: &str,
    leaf_name_counts: &BTreeMap<String, usize>,
    logical_path_counts: &BTreeMap<String, usize>,
) -> Vec<String> {
    let normalized = instrument_name.trim_end_matches('/');
    let mut aliases = BTreeSet::new();
    aliases.insert(instrument_name.to_string());
    aliases.insert(normalized.to_string());

    // The untiered full path is safe unless the same path exists in the other
    // content tier. A shared leaf in a different folder does not make it
    // ambiguous.
    let logical = instrument_logical_name(instrument_name);
    if logical_path_counts.get(logical).copied() == Some(1) {
        aliases.insert(logical.to_string());
        aliases.insert(format!("{logical}/"));
    }

    if let Some(leaf) = instrument_leaf_name(instrument_name) {
        if leaf_name_counts.get(&leaf).copied() == Some(1) {
            // Bare leaf aliases are only safe when no other UI shares the leaf,
            // including a same-named instrument in the other content tier.
            aliases.insert(leaf.clone());
            aliases.insert(format!("{leaf}/"));
        }
    }

    aliases.into_iter().collect()
}

/// Dispatch names for a versioned factory release: its pinned id and every
/// alias that resolves to it, minus unqualified names another instrument's UI
/// also answers to (the counts include both).
fn release_instrument_ui_dispatch_aliases(
    id: &str,
    aliases: &[String],
    leaf_name_counts: &BTreeMap<String, usize>,
    logical_path_counts: &BTreeMap<String, usize>,
) -> Vec<String> {
    let mut out = vec![id.to_string()];
    for alias in aliases {
        let trimmed = alias.trim_end_matches('/');
        let unique = alias.contains(':')
            || if trimmed.contains('/') {
                logical_path_counts.get(trimmed).copied() == Some(1)
            } else {
                leaf_name_counts.get(trimmed).copied() == Some(1)
            };
        if unique && !out.contains(alias) {
            out.push(alias.clone());
        }
    }
    out
}

fn warn_custom_ui_source(path: &str, source: &str) {
    eseqlisp::module_alias_migration::warn_on_old_module_aliases(Path::new(path), source);
}

/// Whether a custom UI source uses a removed binding form or host namespace
/// (eseq-0l17.80), logged and skipped by the caller: every custom UI is
/// spliced into one unit, so its compile error would otherwise drop all of
/// them, not just the stale one (a user-tier instrument still calling
/// `reactive-get`).
fn uses_removed_forms(kind: &str, ui_path: &str, exprs: &[eseqlisp::parser::Expression]) -> bool {
    let removed = eseqlisp::compiler::removed_form_uses(exprs);
    if removed.is_empty() {
        return false;
    }
    eprintln!("{kind} skipped in {ui_path}: {}", removed.join("; "));
    true
}

pub(crate) fn build_custom_instrument_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Parser};

    fn collect(
        dir: &Path,
        root: &Path,
        tier: &str,
        releases: &[sequencer::lisp_host::InstrumentReleaseIds],
        out: &mut Vec<(String, String, String)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            // A factory release folder (the top of a versioned
                            // instrument or one of its `versions/<n>`) is
                            // named by its pinned id, never by its path.
                            let inst_name = match releases.iter().find(|release| release.folder == path) {
                                Some(release) => release.id.clone(),
                                None => format!("{tier}{}/", rel.to_string_lossy().replace('\\', "/")),
                            };
                            out.push((inst_name, ui_path.display().to_string(), src));
                        }
                    }
                }
                collect(&path, root, tier, releases, out);
            }
        }
    }

    let mut ui_sources = Vec::new();
    let paths = sequencer::app_paths::app_paths();
    // Versioned releases dispatch on exactly the ids that load them
    // (docs/instrument-versioning-spec.md): the pin, plus whichever legacy
    // names and top-folder forms resolve to that release.
    let releases = sequencer::lisp_host::instrument_release_ids();
    let release_aliases: BTreeMap<String, Vec<String>> = releases
        .iter()
        .map(|release| (release.id.clone(), release.aliases.clone()))
        .collect();
    for root in paths.instrument_roots() {
        collect(&root.path, &root.path, &root.tier.id_prefix(), &releases, &mut ui_sources);
    }

    let mut functions = r#"
(def custom-ui-string-ends-with? (value suffix)
  (if (< (len value) (len suffix))
    false
    (= (substring value (- (len value) (len suffix)) (len value)) suffix)))
"#
    .to_string();
    let mut dispatch = "false".to_string();
    if let Some((instrument_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources
            .iter_mut()
            .find(|(name, _, _)| {
                name.trim_end_matches('/') == instrument_name.trim_end_matches('/')
                    || (!instrument_name.contains(':')
                        && instrument_logical_name(name) == instrument_logical_name(&instrument_name))
            })
        {
            existing.1 = ui_path;
            existing.2 = src;
        } else {
            ui_sources.push((instrument_name, ui_path, src));
        }
    }

    let mut leaf_name_counts = BTreeMap::new();
    let mut logical_path_counts = BTreeMap::new();
    for (name, _, _) in &ui_sources {
        if let Some(aliases) = release_aliases.get(name) {
            // Count the unqualified names a release answers to, so another
            // tier's same-named instrument does not also claim them.
            for alias in aliases.iter().filter(|alias| !alias.contains(':')) {
                let alias = alias.trim_end_matches('/');
                if alias.contains('/') {
                    *logical_path_counts.entry(alias.to_string()).or_insert(0) += 1;
                } else {
                    *leaf_name_counts.entry(alias.to_string()).or_insert(0) += 1;
                }
            }
            continue;
        }
        if let Some(leaf) = instrument_leaf_name(name) {
            *leaf_name_counts.entry(leaf).or_insert(0) += 1;
        }
        *logical_path_counts.entry(instrument_logical_name(name).to_string()).or_insert(0) += 1;
    }

    // Drafts dispatch on their exact engine name only, and are added after
    // the counts so they never make a saved instrument's short name ambiguous.
    let first_draft = ui_sources.len();
    let drafts = DRAFT_INSTRUMENT_UIS.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
    ui_sources.extend(drafts.into_iter().map(|(name, (path, src))| (name, path, src)));

    for (index, (instrument_name, ui_path, src)) in ui_sources.into_iter().enumerate() {
        let is_draft = index >= first_draft;
        warn_custom_ui_source(&ui_path, &src);
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom instrument UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom instrument UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        if uses_removed_forms("custom instrument UI", &ui_path, &exprs) {
            continue;
        }
        let Some(definitions) = instrument_ui_definitions(&instrument_name, &exprs) else {
            eprintln!(
                "[custom-ui] instrument ui skipped name={instrument_name:?} path={ui_path}: missing defsynth-ui"
            );
            continue;
        };
        let normalized_instrument_name = instrument_name.trim_end_matches('/');
        let fn_name = definitions.fn_name;
        for helper in definitions.helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&format!("\n{}\n", definitions.function));
        let release_alias_list = release_aliases.get(&instrument_name);
        let aliases = if is_draft {
            vec![instrument_name.clone()]
        } else {
            match release_alias_list {
                Some(release_aliases) => {
                    release_instrument_ui_dispatch_aliases(&instrument_name, release_aliases, &leaf_name_counts, &logical_path_counts)
                }
                None => instrument_ui_dispatch_aliases(&instrument_name, &leaf_name_counts, &logical_path_counts),
            }
        };
        let mut name_clauses = aliases
            .iter()
            .map(|alias| format!("(= (get inst :name) {})", lisp_string_literal(alias)))
            .collect::<Vec<_>>();
        if let Some(leaf) = instrument_leaf_name(&instrument_name).filter(|_| release_alias_list.is_none() && !is_draft) {
            if !instrument_name.contains(':') && leaf != normalized_instrument_name
                && leaf_name_counts.get(&leaf).copied() == Some(1)
            {
                name_clauses.push(format!(
                    "(custom-ui-string-ends-with? (get inst :name) {})",
                    lisp_string_literal(&format!("/{leaf}/"))
                ));
                name_clauses.push(format!(
                    "(custom-ui-string-ends-with? (get inst :name) {})",
                    lisp_string_literal(&format!("/{leaf}"))
                ));
            }
        }
        let name_match = if name_clauses.len() == 1 {
            name_clauses.remove(0)
        } else {
            format!("(or {})", name_clauses.join(" "))
        };
        dispatch = format!("(if {name_match} ({fn_name} inst) {dispatch})");
    }

    format!("{functions}\n(def custom-instrument-synth-ui (inst) {dispatch})\n")
}

pub(crate) fn is_generated_custom_ui_source_path(path: &Path) -> bool {
    path.ends_with(GENERATED_INSTRUMENT_UI_PATH)
        || path.ends_with(GENERATED_MIDI_FX_UI_PATH)
        || path.ends_with(GENERATED_AUDIO_FX_UI_PATH)
}

pub(crate) fn build_custom_midi_fx_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Expression, Parser};

    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            let fx_name = rel.to_string_lossy().replace('\\', "/");
                            out.push((fx_name, ui_path.display().to_string(), src));
                        }
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let root = sequencer::app_paths::app_paths().midi_fx_dir();
    let mut ui_sources = Vec::new();
    collect(&root, &root, &mut ui_sources);

    if let Some((fx_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources.iter_mut().find(|(name, _, _)| name == &fx_name) {
            *existing = (fx_name, ui_path, src);
        } else {
            ui_sources.push((fx_name, ui_path, src));
        }
    }

    let mut functions = String::new();
    let mut dispatch = "false".to_string();
    for (fx_name, ui_path, src) in ui_sources {
        warn_custom_ui_source(&ui_path, &src);
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom MIDI FX UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom MIDI FX UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        if uses_removed_forms("custom MIDI FX UI", &ui_path, &exprs) {
            continue;
        }
        let helper_prefix = format!("custom_ui_midi_{}__", safe_lisp_ident(&fx_name));
        let helper_names = local_helper_names(&exprs);
        let mut body = None;
        let mut helpers = Vec::new();
        for expr in &exprs {
            let expr = namespace_local_helpers(expr, &helper_names, &helper_prefix);
            if let Expression::List(items) = &expr {
                if matches!(items.first(), Some(Expression::Symbol(head)) if head == "def-midi-fx-ui")
                {
                    body = items
                        .get(1)
                        .map(|expr| namespace_local_helpers(expr, &helper_names, &helper_prefix))
                        .map(|expr| transform_midi_fx_ui_expr(&expr));
                    continue;
                }
            }
            helpers.push(transform_midi_fx_ui_expr(&expr));
        }
        let Some(body) = body else {
            continue;
        };
        let fn_name = format!("custom-midi-fx-ui-{}", safe_lisp_ident(&fx_name));
        for helper in helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&effect_ui_function_source(
            &fn_name,
            "midi-fx",
            &lisp_string_literal(&fx_name),
            &body.to_string(),
        ));
        dispatch = format!(
            "(if (or (= (get fx :name) {}) (= (get fx :name) {})) ({fn_name} fx) {dispatch})",
            lisp_string_literal(&fx_name),
            lisp_string_literal(&format!("{}/", fx_name.trim_end_matches('/')))
        );
    }

    format!("{functions}\n(def custom-midi-fx-ui (fx) {dispatch})\n")
}

pub(crate) fn build_custom_audio_fx_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Expression, Parser};

    fn collect(dir: &Path, root: &Path, prefix: &str, out: &mut Vec<(String, String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            let fx_name =
                                format!("{prefix}{}", rel.to_string_lossy().replace('\\', "/"));
                            out.push((fx_name, ui_path.display().to_string(), src));
                        }
                    }
                    // A folder effect owns everything beneath it (captures,
                    // research snapshots, helper Lisp); none of it is an
                    // effect, matching `list_saved_effects_in_roots`.
                    continue;
                }
                collect(&path, root, prefix, out);
            }
        }
    }

    let mut ui_sources = Vec::new();
    for root in sequencer::app_paths::app_paths().effect_roots() {
        // Library effects keep their bare names; package effects carry their
        // `pkg:author.name/` qualifier, which is the name the graph binds.
        let prefix = if root.tier.is_package() { root.tier.id_prefix() } else { String::new() };
        collect(&root.path, &root.path, &prefix, &mut ui_sources);
    }

    if let Some((fx_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources.iter_mut().find(|(name, _, _)| name == &fx_name) {
            *existing = (fx_name, ui_path, src);
        } else {
            ui_sources.push((fx_name, ui_path, src));
        }
    }

    let mut functions = String::new();
    let mut dispatch = "false".to_string();
    for (fx_name, ui_path, src) in ui_sources {
        warn_custom_ui_source(&ui_path, &src);
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom audio effect UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom audio effect UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        if uses_removed_forms("custom audio effect UI", &ui_path, &exprs) {
            continue;
        }
        let helper_prefix = format!("custom_ui_audio_{}__", safe_lisp_ident(&fx_name));
        let helper_names = local_helper_names(&exprs);
        let mut body = None;
        let mut helpers = Vec::new();
        for expr in &exprs {
            let expr = namespace_local_helpers(expr, &helper_names, &helper_prefix);
            if let Expression::List(items) = &expr {
                if matches!(items.first(), Some(Expression::Symbol(head)) if head == "defeffect-ui")
                {
                    body = items.get(1).map(transform_audio_fx_ui_expr);
                    continue;
                }
            }
            helpers.push(transform_audio_fx_ui_expr(&expr));
        }
        let Some(body) = body else {
            eprintln!(
                "[custom-ui] audio effect ui skipped name={fx_name:?} path={ui_path}: missing defeffect-ui"
            );
            continue;
        };
        let fn_name = format!("custom-audio-fx-ui-{}", safe_lisp_ident(&fx_name));
        for helper in helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&effect_ui_function_source(
            &fn_name,
            "audio-fx",
            &lisp_string_literal(&fx_name),
            &body.to_string(),
        ));
        dispatch = format!(
            "(if (or (= (get fx :name) {}) (= (get fx :name) {})) ({fn_name} fx) {dispatch})",
            lisp_string_literal(&fx_name),
            lisp_string_literal(&format!("{}/", fx_name.trim_end_matches('/')))
        );
    }

    format!("{functions}\n(def custom-audio-fx-ui (fx) {dispatch})\n")
}

/// The generated custom instrument, MIDI FX and audio effect UI sources,
/// built from disk. Building reads no editor or runtime state beyond the
/// overlays, so boot builds them on a worker while the UI entrypoint
/// evaluates ([`CustomUiSources::spawn_build`]).
pub(crate) struct CustomUiSources {
    instrument: String,
    midi_fx: String,
    audio_fx: String,
}

type CustomUiOverlays = [Option<(String, String, String)>; 3];

impl CustomUiSources {
    fn overlays(editor: &Editor) -> CustomUiOverlays {
        [
            active_custom_ui_buffer_overlay(editor),
            active_custom_midi_fx_ui_buffer_overlay(editor),
            active_custom_audio_fx_ui_buffer_overlay(editor),
        ]
    }

    fn build([instrument, midi_fx, audio_fx]: CustomUiOverlays) -> Self {
        Self {
            instrument: build_custom_instrument_ui_source_with_overlay(instrument),
            midi_fx: build_custom_midi_fx_ui_source_with_overlay(midi_fx),
            audio_fx: build_custom_audio_fx_ui_source_with_overlay(audio_fx),
        }
    }

    pub(crate) fn spawn_build(editor: &Editor) -> std::thread::JoinHandle<Self> {
        let overlays = Self::overlays(editor);
        std::thread::Builder::new()
            .name("custom-ui-build".into())
            .spawn(move || Self::build(overlays))
            .expect("spawn custom UI source build")
    }

    pub(crate) fn load(self, editor: &mut Editor) -> bool {
        let mut ok = true;
        for (source, path, label) in [
            (self.instrument, GENERATED_INSTRUMENT_UI_PATH, "custom instrument UI"),
            (self.midi_fx, GENERATED_MIDI_FX_UI_PATH, "custom MIDI FX UI"),
            (self.audio_fx, GENERATED_AUDIO_FX_UI_PATH, "custom audio effect UI"),
        ] {
            if !source.is_empty() {
                ok &= reload_custom_ui_source(editor, path, &source, label);
            }
        }
        ok
    }
}

pub(crate) fn reload_custom_instrument_ui(editor: &mut Editor) -> bool {
    CustomUiSources::build(CustomUiSources::overlays(editor)).load(editor)
}

/// Rebuild only the instrument panels (the MIDI FX and audio effect ones are
/// untouched), for a change that can only affect an instrument's.
pub(crate) fn reload_instrument_panels(editor: &mut Editor) -> bool {
    let source = build_custom_instrument_ui_source_with_overlay(active_custom_ui_buffer_overlay(editor));
    source.is_empty()
        || reload_custom_ui_source(editor, GENERATED_INSTRUMENT_UI_PATH, &source, "custom instrument UI")
}

fn reload_custom_ui_source(editor: &mut Editor, path: &str, source: &str, label: &str) -> bool {
    let report = editor.runtime_mut().eval_source_transactional(
        Some(PathBuf::from(path)),
        source,
        Vec::new(),
    );
    let success = report.success;
    if !success {
        for diagnostic in &report.diagnostics {
            eprintln!("{label} load error: {diagnostic}");
        }
    }
    editor.process_lisp_reload_report(report);
    success
}

#[cfg(test)]
mod tests {
    use super::{
        build_custom_audio_fx_ui_source_with_overlay,
        build_custom_instrument_ui_source_with_overlay, build_custom_midi_fx_ui_source_with_overlay,
        instrument_ui_dispatch_aliases, release_instrument_ui_dispatch_aliases,
        reload_custom_ui_source, set_draft_instrument_uis, GENERATED_INSTRUMENT_UI_PATH,
    };
    use eseqlisp::vm::Value;
    use std::collections::BTreeMap;

    fn value_contains_string(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => {
                value.contains(needle)
            }
            Value::List(items) => items
                .iter()
                .any(|item| value_contains_string(&item.borrow(), needle)),
            Value::Map(map) => map.iter().any(|(key, value)| {
                key.contains(needle) || value_contains_string(&value.borrow(), needle)
            }),
            _ => false,
        }
    }

    #[test]
    fn authored_custom_ui_bypasses_run_module_alias_preflight() {
        let cases = [
            (
                "instrument",
                "instruments/external/ui.lisp",
                "(def helper () (eseq.seq-layout/apply-fx-layout))\n(defsynth-ui (label \"ok\"))",
            ),
            (
                "midi-fx",
                "midi-fx/external/ui.lisp",
                "(def helper () (eseq.seq-layout/apply-fx-layout))\n(def-midi-fx-ui (label \"ok\"))",
            ),
            (
                "audio-fx",
                "effects/external/ui.lisp",
                "(def helper () (eseq.seq-layout/apply-fx-layout))\n(defeffect-ui (label \"ok\"))",
            ),
        ];
        build_custom_instrument_ui_source_with_overlay(Some((
            "external/".to_string(), cases[0].1.to_string(), cases[0].2.to_string(),
        )));
        build_custom_midi_fx_ui_source_with_overlay(Some((
            "external".to_string(), cases[1].1.to_string(), cases[1].2.to_string(),
        )));
        build_custom_audio_fx_ui_source_with_overlay(Some((
            "external".to_string(), cases[2].1.to_string(), cases[2].2.to_string(),
        )));
        for (kind, path, source) in cases {
            assert!(
                eseqlisp::module_alias_migration::warn_on_old_module_aliases(
                    std::path::Path::new(path), source,
                ).is_none(),
                "{kind} authored source was not preflighted before transformation",
            );
        }
    }

    #[test]
    fn local_widget_named_by_string_follows_its_namespaced_definition() {
        use eseqlisp::parser::{ASTParser, Parser};
        use super::{local_helper_names, expr_to_lisp, namespace_local_helpers};
        let authored = r#"
            (def panel-bg () 1)
            (defwidget bg :width 4 :height 2 :shader (sdf/layer (sdf/fill (sdf/rect width height) :red)))
            (box :background "bg" :icon "bg" :label "bg"
              (box :background "pattern-pill-bg")
              (box :background "panel-bg"))
        "#;
        let exprs = ASTParser::new(Parser::new(authored.to_string()).parse().unwrap()).parse().unwrap();
        let helpers = local_helper_names(&exprs);
        let source = exprs.iter()
            .map(|expr| expr_to_lisp(&namespace_local_helpers(expr, &helpers, "fx_")))
            .collect::<Vec<_>>().join("\n");
        assert!(source.contains("(defwidget fx_bg"), "{source}");
        assert!(source.contains(r#":background "fx_bg""#), "{source}");
        assert!(source.contains(r#":icon "fx_bg""#), "{source}");
        // Other props, global widgets and non-widget helpers keep their strings.
        assert!(source.contains(r#":label "bg""#), "{source}");
        assert!(source.contains(r#":background "pattern-pill-bg""#), "{source}");
        assert!(source.contains(r#":background "panel-bg""#), "{source}");
    }

    #[test]
    fn instrument_ui_namespaces_nested_sdf_macros_with_reactive_uniforms() {
        use eseqlisp::parser::{ASTParser, Parser};
        use eseqlisp::Runtime;
        use eseqlisp::vm::Value;
        use super::{local_helper_names, expr_to_lisp, namespace_local_helpers};
        let authored = r#"
            (defmacro contour (v) `(* ,v 0.8))
            (defmacro shape (v) `(sdf/circle (contour ,v)))
            (defwidget plot :width 8 :height 3
              :state (level)
              :shader (sdf/layer (sdf/stroke (shape level) 0.04 :dim)))
        "#;
        let exprs = ASTParser::new(Parser::new(authored.to_string()).parse().unwrap()).parse().unwrap();
        let helpers = local_helper_names(&exprs);
        let source = exprs.iter().map(|expr| {
            expr_to_lisp(&namespace_local_helpers(expr, &helpers, "test_heat_"))
        }).collect::<Vec<_>>().join("\n");
        let mut runtime = Runtime::new();
        runtime.register_reactive("HEAT", vec![("level", Value::Number(0.25))], true);
        let result = runtime.eval_str(&source).unwrap();
        assert!(!matches!(result, Some(Value::String(ref error)) if error.contains("error")), "{result:?}");
        runtime.eval_str(r#"(effect (test_heat_plot :level #'HEAT.level))"#).unwrap();
        let layout = runtime.current_layout.as_ref().expect("SDF plot");
        assert_eq!(layout.widget_type, "test_heat_plot");
        let Some(Value::ReactiveRef { slot, .. }) = layout.props.get("level") else {
            panic!("the authored parameter must remain a reactive binding: {:?}", layout.props);
        };
        let slot = slot.clone();
        assert_eq!(f64::from_bits(slot.load(std::sync::atomic::Ordering::Relaxed)), 0.25);
        runtime.set_reactive("HEAT", "level", Value::Number(0.8));
        runtime.run_reactive_cycle();
        assert_eq!(f64::from_bits(slot.load(std::sync::atomic::Ordering::Relaxed)), 0.8);
        assert_eq!(runtime.current_layout.as_ref().unwrap().widget_type, "test_heat_plot");
    }

    #[test]
    fn draft_instrument_ui_dispatches_on_its_exact_engine_name() {
        set_draft_instrument_uis(BTreeMap::from([(
            "draft/probe-draft-ui".to_string(),
            ("probe/ui.lisp".to_string(), "(defsynth-ui (label \"draft panel\"))".to_string()),
        )]));
        let source = build_custom_instrument_ui_source_with_overlay(None);
        set_draft_instrument_uis(BTreeMap::new());
        assert!(source.contains("draft panel"), "{source}");
        assert!(source.contains(r#"(= (get inst :name) "draft/probe-draft-ui")"#), "{source}");
        // No short-name match: a saved probe-draft-ui keeps its own panel.
        assert!(!source.contains(r#"(= (get inst :name) "probe-draft-ui")"#), "{source}");
        assert!(!source.contains(r#""/probe-draft-ui""#), "{source}");
        assert!(!build_custom_instrument_ui_source_with_overlay(None).contains("draft panel"));
    }

    #[test]
    fn instrument_ui_injection_namespaces_local_helpers() {
        let source = build_custom_instrument_ui_source_with_overlay(Some((
            "agent-draft-1/".to_string(),
            "instruments/agent-draft-1/ui.lisp".to_string(),
            r#"
            (def tune-block ()
              (eseq.effects.custom-ui-lego/ui-control-block-small-s "TUNE" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
                (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "op1_ratio" "r1" 4.1 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))

            (defsynth-ui
              (eseq.effects.custom-ui-lego/ui-lego-column-full
                (tune-block)))
            "#
            .to_string(),
        )));

        assert!(
            source.contains("(def custom_ui_agent_draft_1___tune_block"),
            "{source}"
        );
        assert!(
            source.contains("(custom_ui_agent_draft_1___tune_block)"),
            "{source}"
        );
        assert!(
            !source.contains("(def tune-block"),
            "local helper should not be injected under its unqualified name:\n{source}"
        );
    }

    #[test]
    fn instrument_ui_dispatch_matches_unique_moved_folder_leaf_name() {
        let source = build_custom_instrument_ui_source_with_overlay(Some((
            "emulations/unique-dispatch-test-voice/".to_string(),
            "instruments/emulations/unique-dispatch-test-voice/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (eseq.effects.custom-ui-lego/ui-lego-column-full
                (eseq.effects.custom-ui-lego/ui-control-block-small-s "OSC" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))
            "#
            .to_string(),
        )));

        assert!(
            source.contains(r#"(= (get inst :name) "emulations/unique-dispatch-test-voice/")"#),
            "{source}"
        );
        assert!(
            source.contains(r#"(= (get inst :name) "emulations/unique-dispatch-test-voice")"#),
            "{source}"
        );
        assert!(
            source.contains(r#"(= (get inst :name) "unique-dispatch-test-voice/")"#),
            "{source}"
        );
        assert!(
            source.contains(r#"(= (get inst :name) "unique-dispatch-test-voice")"#),
            "{source}"
        );
        assert!(
            source.contains(r#"(custom-ui-string-ends-with? (get inst :name) "/unique-dispatch-test-voice/")"#),
            "{source}"
        );
    }

    #[test]
    fn instrument_ui_dispatch_does_not_alias_ambiguous_leaf_name() {
        let mut counts = BTreeMap::new();
        counts.insert("shared-name".to_string(), 2);
        let mut paths = BTreeMap::new();
        paths.insert("folder-a/shared-name".to_string(), 1);
        paths.insert("folder-b/shared-name".to_string(), 1);

        let aliases = instrument_ui_dispatch_aliases("folder-a/shared-name/", &counts, &paths);

        assert!(aliases.contains(&"folder-a/shared-name/".to_string()));
        assert!(aliases.contains(&"folder-a/shared-name".to_string()));
        assert!(!aliases.contains(&"shared-name/".to_string()));
        assert!(!aliases.contains(&"shared-name".to_string()));

        // A tiered id keeps its untiered full path when only the leaf collides.
        let aliases = instrument_ui_dispatch_aliases("factory:folder-a/shared-name/", &counts, &paths);
        assert!(aliases.contains(&"folder-a/shared-name/".to_string()));
        assert!(aliases.contains(&"folder-a/shared-name".to_string()));
        assert!(!aliases.contains(&"shared-name".to_string()));
    }

    #[test]
    fn release_ui_dispatch_answers_to_its_pin_and_uncontested_legacy_names() {
        let aliases = [
            "factory:Synths/Drift",
            "Synths/Drift",
            "Synths/Drift/",
            "Drift",
        ]
        .map(String::from);
        let mut leaves = BTreeMap::new();
        leaves.insert("Drift".to_string(), 1);
        let mut paths = BTreeMap::new();
        paths.insert("Synths/Drift".to_string(), 1);
        let dispatch =
            release_instrument_ui_dispatch_aliases("factory:Synths/Syn@1", &aliases, &leaves, &paths);
        assert_eq!(dispatch[0], "factory:Synths/Syn@1");
        for alias in &aliases {
            assert!(dispatch.contains(alias), "{alias}");
        }
        // A user instrument sharing the legacy path or leaf contests the
        // unqualified names; the qualified ones stay.
        leaves.insert("Drift".to_string(), 2);
        paths.insert("Synths/Drift".to_string(), 2);
        let dispatch =
            release_instrument_ui_dispatch_aliases("factory:Synths/Syn@1", &aliases, &leaves, &paths);
        assert_eq!(dispatch, ["factory:Synths/Syn@1", "factory:Synths/Drift"]);
    }

    #[test]
    fn instrument_ui_dispatch_preserves_content_tier_for_root_level_names() {
        let mut counts = BTreeMap::new();
        counts.insert("Heat".to_string(), 1);
        let mut paths = BTreeMap::new();
        paths.insert("Heat".to_string(), 1);
        let aliases = instrument_ui_dispatch_aliases("user:Heat/", &counts, &paths);
        assert!(aliases.contains(&"user:Heat".to_string()));
        assert!(aliases.contains(&"user:Heat/".to_string()));
        assert!(aliases.contains(&"Heat".to_string()));
        assert!(!aliases.contains(&"factory:Heat".to_string()));

        counts.insert("Heat".to_string(), 2);
        paths.insert("Heat".to_string(), 2);
        for tier in ["factory", "user"] {
            let name = format!("{tier}:Heat");
            let aliases = instrument_ui_dispatch_aliases(&format!("{name}/"), &counts, &paths);
            assert_eq!(aliases, vec![name.clone(), format!("{name}/")]);
        }
    }

    #[test]
    fn audio_effect_ui_dispatch_matches_folder_names_with_trailing_slash() {
        let source = build_custom_audio_fx_ui_source_with_overlay(Some((
            "agent-effect-draft-1".to_string(),
            "effects/agent-effect-draft-1/ui.lisp".to_string(),
            r#"
            (defeffect-ui
              (eseq.effects.custom-ui-lego/ui-lego-column-full
                (eseq.effects.custom-ui-lego/ui-control-block-small-s "MIX" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))
            "#
            .to_string(),
        )));

        assert!(
            source.contains(r#"(= (get fx :name) "agent-effect-draft-1")"#),
            "{source}"
        );
        assert!(
            source.contains(r#"(= (get fx :name) "agent-effect-draft-1/")"#),
            "{source}"
        );
    }

    #[test]
    fn custom_ui_reload_rerenders_existing_effect_buffer_body() {
        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        assert!(reload_custom_ui_source(
            &mut editor,
            GENERATED_INSTRUMENT_UI_PATH,
            r#"(def custom-instrument-synth-ui (inst) (label "old custom ui"))"#,
            "test custom instrument UI"
        ));
        editor
            .runtime_mut()
            .eval_str(
                r#"(effect-buffer "*custom-ui-hot-reload-test*"
                      (custom-instrument-synth-ui (dict :name "test-instrument")))"#,
            )
            .expect("create test effect buffer");
        editor.refresh_runtime_side_effects();

        let buffer_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.name == "*custom-ui-hot-reload-test*")
            .expect("effect buffer should exist");
        // Hidden named buffers defer reactive reruns (eseq-md1n.4); present
        // the target so the reload keeps its live-rerender contract.
        let buffer_id = editor.buffers[buffer_idx].id;
        editor.set_active_buffer(buffer_id);
        let tree = editor.buffers[buffer_idx]
            .widget_tree
            .as_ref()
            .expect("effect buffer should have initial widget tree");
        assert!(value_contains_string(tree, "old custom ui"));

        assert!(reload_custom_ui_source(
            &mut editor,
            GENERATED_INSTRUMENT_UI_PATH,
            r#"(def custom-instrument-synth-ui (inst) (label "new custom ui"))"#,
            "test custom instrument UI"
        ));

        let tree = editor.buffers[buffer_idx]
            .widget_tree
            .as_ref()
            .expect("effect buffer should keep widget tree after reload");
        assert!(
            value_contains_string(tree, "new custom ui"),
            "custom UI reload should rerender existing effect buffer"
        );
        assert!(!value_contains_string(tree, "old custom ui"));
    }
}

/// Whether the active buffer is an instrument / MIDI FX / audio effect panel
/// source (`ui.lisp` beside a `dsp.lisp` under a library root) — the only
/// buffers whose evaluation needs the custom UI dispatch rebuilt.
pub(crate) fn active_buffer_is_custom_ui_source(editor: &Editor) -> bool {
    active_custom_ui_buffer_overlay(editor).is_some()
        || active_custom_midi_fx_ui_buffer_overlay(editor).is_some()
        || active_custom_audio_fx_ui_buffer_overlay(editor).is_some()
}

fn active_custom_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if !buffer.dirty && !path.is_file() { return None; }
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let (prefix, rel) = sequencer::app_paths::app_paths()
        .instrument_roots()
        .into_iter()
        .find_map(|root| {
            folder
                .strip_prefix(&root.path)
                .ok()
                .map(|rel| (root.tier.id_prefix(), rel.to_path_buf()))
        })?;
    // A release folder's UI is dispatched under its pinned id.
    let instrument_name = sequencer::lisp_host::instrument_release_id_for_folder(folder)
        .unwrap_or_else(|| format!("{prefix}{}/", rel.to_string_lossy().replace('\\', "/")));
    Some((instrument_name, path.display().to_string(), buffer.text()))
}

fn active_custom_midi_fx_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if !buffer.dirty && !path.is_file() { return None; }
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let root = sequencer::app_paths::app_paths().midi_fx_dir();
    let rel = folder.strip_prefix(root).ok()?;
    let fx_name = rel.to_string_lossy().replace('\\', "/");
    Some((fx_name, path.display().to_string(), buffer.text()))
}

fn active_custom_audio_fx_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if !buffer.dirty && !path.is_file() { return None; }
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let (prefix, rel) = sequencer::app_paths::app_paths()
        .effect_roots()
        .into_iter()
        .find_map(|root| {
            folder.strip_prefix(&root.path).ok().map(|rel| {
                let prefix = if root.tier.is_package() { root.tier.id_prefix() } else { String::new() };
                (prefix, rel.to_path_buf())
            })
        })?;
    let fx_name = format!("{prefix}{}", rel.to_string_lossy().replace('\\', "/"));
    Some((fx_name, path.display().to_string(), buffer.text()))
}
