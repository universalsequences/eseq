use std::path::{Path, PathBuf};

use sequencer::agent::verify::{verify_sources, ArtifactKind, VerifyMode};
use sequencer::app_paths::ContentTier;

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("eseq: {error}");
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: eseq paths
       eseq authoring seed [DIR]
       eseq authoring skill
       eseq instrument check DIR_OR_NAME [--no-render]
       eseq effect check DIR_OR_NAME [--no-render]
       eseq sequencer check MODULE [--no-render] [--eval FORM]
       eseq package index [PACKAGE_DIR]
       eseq package install AUTHOR/NAME GIT_URL
       eseq package import PATH_OR_ARCHIVE
       eseq package export AUTHOR/NAME VERSION OUT_DIR (instrument:<name>|effect:<name>|presets:<factory-instrument>)...";

/// Audition rate for `check`; matches the default device rate.
const CHECK_SAMPLE_RATE: u32 = 48_000;

fn run(args: Vec<String>) -> Result<(), String> {
    // Select the bundle layout when running from ESeq.app/Contents/MacOS.
    // Without this the lazy accessor falls back to the build machine's
    // checkout paths.
    sequencer::app_paths::init()
        .map_err(|error| format!("failed to resolve application paths: {error}"))?;
    match args.as_slice() {
        [paths] if paths == "paths" => print_paths(),
        [authoring, seed, dir @ ..] if authoring == "authoring" && seed == "seed" && dir.len() <= 1 => {
            let paths = sequencer::app_paths::app_paths();
            paths
                .ensure_user_tier()
                .map_err(|error| format!("failed to initialize user content directories: {error}"))?;
            let root = dir.first().map(PathBuf::from).unwrap_or_else(|| paths.user_data_root());
            let cli = sequencer::authoring_kit::eseq_cli_path();
            for outcome in sequencer::authoring_kit::seed(paths, &root, &cli)
                .map_err(|error| format!("failed to write the authoring guide: {error}"))?
            {
                match outcome {
                    sequencer::authoring_kit::SeedOutcome::Written(path) => {
                        println!("wrote {}", path.display())
                    }
                    sequencer::authoring_kit::SeedOutcome::KeptUserFile(path) => {
                        println!("kept  {} (user-edited)", path.display())
                    }
                }
            }
            Ok(())
        }
        [authoring, skill] if authoring == "authoring" && skill == "skill" => {
            let paths = sequencer::app_paths::app_paths();
            print!(
                "{}",
                sequencer::authoring_kit::skill_md(paths, &sequencer::authoring_kit::eseq_cli_path())
            );
            Ok(())
        }
        [kind, check, target, flags @ ..]
            if (kind == "instrument" || kind == "effect") && check == "check" =>
        {
            let kind = if kind == "instrument" {
                ArtifactKind::Instrument
            } else {
                ArtifactKind::Effect
            };
            let render = match flags {
                [] => true,
                [flag] if flag == "--no-render" => false,
                _ => return Err(USAGE.to_string()),
            };
            check_artifact(kind, target, render)
        }
        [sequencer, check, module, flags @ ..]
            if sequencer == "sequencer" && check == "check" =>
        {
            let mut render = true;
            let mut eval = None;
            let mut rest = flags.iter();
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--no-render" => render = false,
                    "--eval" => eval = Some(rest.next().ok_or(USAGE)?.clone()),
                    _ => return Err(USAGE.to_string()),
                }
            }
            check_sequencer(module, render, eval.as_deref())
        }
        [package, index] if package == "package" && index == "index" => {
            index_package(std::env::current_dir().map_err(|error| error.to_string())?)
        }
        [package, index, path] if package == "package" && index == "index" => {
            index_package(PathBuf::from(path))
        }
        [package, install, identity, repository]
            if package == "package" && install == "install" =>
        {
            let app_paths = sequencer::app_paths::app_paths();
            app_paths.ensure_user_tier().map_err(|error| {
                format!("failed to initialize user content directories: {error}")
            })?;
            let result = sequencer::package_install::install_git_package(
                repository,
                identity,
                &app_paths.packages_dir(),
            )?;
            let report = sequencer::package_samples::reconcile_app_package_samples(app_paths)?;
            println!("installed {} at {}", result.identity, result.path.display());
            for error in report.errors {
                eprintln!("eseq: {error}");
            }
            Ok(())
        }
        [package, import, path] if package == "package" && import == "import" => {
            let app_paths = sequencer::app_paths::app_paths();
            app_paths.ensure_user_tier().map_err(|error| {
                format!("failed to initialize user content directories: {error}")
            })?;
            let staged = sequencer::package_install::stage_package_from_path(
                std::path::Path::new(path),
                &app_paths.packages_dir(),
            )?;
            let replace = staged.replaces_installed();
            let summary = staged.summary.clone();
            let result = sequencer::package_install::publish_staged_package(staged, replace)?;
            let report = sequencer::package_samples::reconcile_app_package_samples(app_paths)?;
            println!(
                "{} {} {} at {} ({})",
                if replace { "replaced" } else { "installed" },
                summary.identity,
                summary.version,
                result.path.display(),
                summary.describe_contents()
            );
            for error in report.errors {
                eprintln!("eseq: {error}");
            }
            Ok(())
        }
        [package, export, identity, version, out_dir, items @ ..]
            if package == "package" && export == "export" =>
        {
            use sequencer::package_export::{export_package, ExportKind, ExportRequest};
            let mut picked = Vec::new();
            for item in items {
                let (kind, name) = item.split_once(':').ok_or_else(|| {
                    format!("item `{item}` must be instrument:<name> or effect:<name>")
                })?;
                let kind = ExportKind::parse(kind)
                    .ok_or_else(|| format!("unknown item kind `{kind}`"))?;
                picked.push((kind, name.to_string()));
            }
            let request = ExportRequest {
                identity: identity.clone(),
                version: version.clone(),
                items: picked,
            };
            let report = export_package(
                sequencer::app_paths::app_paths(),
                &request,
                std::path::Path::new(out_dir),
                true,
            )?;
            println!(
                "exported {} ({} instrument(s), {} effect(s), {} preset bank(s))",
                report.archive.as_deref().unwrap_or(&report.package_dir).display(),
                report.instruments,
                report.effects,
                report.presets
            );
            for warning in report.warnings {
                eprintln!("eseq: warning: {warning}");
            }
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

fn print_paths() -> Result<(), String> {
    let paths = sequencer::app_paths::app_paths();
    paths
        .ensure_user_tier()
        .map_err(|error| format!("failed to initialize user content directories: {error}"))?;
    println!("{:<20}{}", "user-root", paths.user_data_root().display());
    println!("{:<20}{}", "user-instruments", paths.user_instruments_dir().display());
    println!("{:<20}{}", "user-effects", paths.user_effects_dir().display());
    println!("{:<20}{}", "factory-instruments", paths.instruments_dir().display());
    println!("{:<20}{}", "factory-effects", paths.effects_dir().display());
    println!("{:<20}{}", "packages", paths.packages_dir().display());
    println!("{:<20}{}", "local-packages", paths.local_modules_dir().display());
    println!("{:<20}{}", "authoring", paths.authoring_dir().display());
    println!("{:<20}{}", "dgenlisp", paths.dgenlisp_tool().display());
    Ok(())
}

struct CheckSources {
    label: String,
    dsp_path: PathBuf,
    ui_path: PathBuf,
    /// The id the host loads this by (`user:bass/`, `Synths/Heat`), when the
    /// sources live in a library root; the panel render needs it.
    library_id: Option<String>,
}

/// A directory holding `dsp.lisp` (and usually `ui.lisp`), a path to that
/// `dsp.lisp`, or a library name such as `Synths/Heat` or `user:bass`.
fn resolve_check_target(kind: ArtifactKind, target: &str) -> Result<CheckSources, String> {
    let path = Path::new(target);
    let dir = if path.is_dir() {
        Some(path.to_path_buf())
    } else if path.is_file() && path.file_name().is_some_and(|name| name == "dsp.lisp") {
        path.parent().map(Path::to_path_buf)
    } else {
        None
    };
    if let Some(dir) = dir {
        let dir = std::fs::canonicalize(&dir)
            .map_err(|error| format!("cannot resolve {}: {error}", dir.display()))?;
        return Ok(CheckSources {
            label: dir.display().to_string(),
            dsp_path: dir.join("dsp.lisp"),
            ui_path: dir.join("ui.lisp"),
            library_id: library_id_for_dir(kind, &dir),
        });
    }
    let (dsp_path, ui_path) = match kind {
        ArtifactKind::Instrument => (
            sequencer::lisp_host::instrument_source_path(target)
                .map_err(|error| format!("instrument `{target}` not found: {error}"))?,
            sequencer::lisp_host::instrument_ui_path(target)
                .map_err(|error| format!("instrument `{target}` panel not found: {error}"))?,
        ),
        ArtifactKind::Effect => (
            sequencer::lisp_host::effect_source_path(target),
            sequencer::lisp_host::effect_ui_path(target),
        ),
    };
    Ok(CheckSources {
        label: target.to_string(),
        dsp_path,
        ui_path,
        library_id: Some(target.to_string()),
    })
}

fn library_id_for_dir(kind: ArtifactKind, dir: &Path) -> Option<String> {
    let paths = sequencer::app_paths::app_paths();
    let roots = match kind {
        ArtifactKind::Instrument => paths.instrument_roots(),
        ArtifactKind::Effect => paths.effect_roots(),
    };
    roots.into_iter().find_map(|root| {
        let root_path = std::fs::canonicalize(&root.path).ok()?;
        let logical = dir.strip_prefix(&root_path).ok()?.to_str()?;
        if logical.is_empty() {
            return None;
        }
        // Effect lookup takes a bare folder name for the factory and user
        // tiers; only a package effect is tier-qualified.
        Some(match (kind, &root.tier) {
            (ArtifactKind::Effect, ContentTier::Factory | ContentTier::User) => format!("{logical}/"),
            _ => format!("{}/", root.tier.qualify(logical)),
        })
    })
}

fn check_artifact(kind: ArtifactKind, target: &str, render: bool) -> Result<(), String> {
    let sources = resolve_check_target(kind, target)?;
    let dsp_source = std::fs::read_to_string(&sources.dsp_path)
        .map_err(|error| format!("cannot read {}: {error}", sources.dsp_path.display()))?;
    let ui_source = match std::fs::read_to_string(&sources.ui_path) {
        Ok(source) => Some(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!("cannot read {}: {error}", sources.ui_path.display()))
        }
    };
    let asset_base = sources.dsp_path.parent();
    let report = verify_sources(
        kind,
        VerifyMode::Library,
        &dsp_source,
        ui_source.as_deref(),
        CHECK_SAMPLE_RATE,
        asset_base,
    )
    .map_err(|failure| format!("FAIL {} ({})\n{failure}", sources.label, failure.stage.label()))?;
    let manifest = &report.compile.manifest;
    println!(
        "ok    compile   {} param(s), {} input(s), {} output(s)",
        // `__mod__…` cells are host modulation plumbing, not the author's params.
        manifest.params.iter().filter(|param| !param.name.starts_with("__")).count(),
        manifest.n_inputs,
        manifest.n_outputs
    );
    if report.ui_checked {
        println!("ok    ui.lisp   structure and param names");
    } else {
        println!("--    ui.lisp   none; the host generates a default panel");
    }
    println!("ok    {}", report.feedback);
    for warning in &report.warnings {
        println!("warn  {}", warning.replace('\n', "\n      "));
    }
    if render {
        render_panel(kind, &sources)?;
    }
    println!("PASS {}", sources.label);
    Ok(())
}

/// Draw the panel with the app's own renderer (`metal_seq capture`, next to
/// this binary) so helper modules, layout and runtime errors are exactly what
/// the app would hit. Prints the PNG path for the author to look at.
fn render_panel(kind: ArtifactKind, sources: &CheckSources) -> Result<(), String> {
    // eseqlisp strings have no escapes, so an id with a quote can't be named.
    let Some(library_id) = sources.library_id.as_ref().filter(|id| !id.contains('"')) else {
        println!(
            "--    panel     not rendered: only content inside an instrument or effect folder \
             of the library can be loaded (see `eseq paths`)"
        );
        return Ok(());
    };
    let metal_seq = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("metal_seq")))
        .filter(|path| path.exists());
    let Some(metal_seq) = metal_seq else {
        println!("--    panel     not rendered: metal_seq not found next to eseq");
        return Ok(());
    };

    let work_dir = std::env::temp_dir().join("eseq-check");
    std::fs::create_dir_all(&work_dir)
        .map_err(|error| format!("cannot create {}: {error}", work_dir.display()))?;
    let stem: String = library_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let script_path = work_dir.join(format!("{stem}.lisp"));
    let png_path = work_dir.join(format!("{stem}.png"));
    let id = format!("\"{library_id}\"");
    let (script, width) = match kind {
        ArtifactKind::Instrument => (format!("(capture-project (track :instrument {id}))\n"), 1800),
        // The effect panel sits to the right of the track's sampler panel.
        ArtifactKind::Effect => (
            format!("(capture-project (track :sampler :audio-fx ({id})))\n"),
            2600,
        ),
    };
    std::fs::write(&script_path, script)
        .map_err(|error| format!("cannot write {}: {error}", script_path.display()))?;
    let output = std::process::Command::new(&metal_seq)
        .arg("capture")
        .arg("--script")
        .arg(&script_path)
        .args(["--buffer", "fx", "--track", "0", "--hide-status"])
        .args(["--width", &width.to_string(), "--height", "520"])
        .arg("--out")
        .arg(&png_path)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", metal_seq.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let ui_path = sources.ui_path.display().to_string();
    let mut own = Vec::new();
    let mut environment = Vec::new();
    for line in stderr.lines() {
        // Module-export warnings come from eseq's own UI modules, not the
        // panel under test.
        if line.contains("is not exported by") {
            continue;
        }
        if line.contains(&ui_path) || line.contains("[lisp-error]") {
            own.push(line);
        } else if line.contains(" load error: ") {
            environment.push(line);
        }
    }
    if !own.is_empty() || (!output.status.success() && environment.is_empty()) {
        let detail = if own.is_empty() {
            stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n")
        } else {
            own.join("\n")
        };
        return Err(format!("FAIL {} (panel render)\n{detail}", sources.label));
    }
    if !environment.is_empty() {
        return Err(format!(
            "FAIL {} (panel render: eseq's own UI failed to load; not caused by these files)\n{}\n\
             The instrument itself passed every other stage. Report this to the user; \
             `--no-render` skips the panel render.",
            sources.label,
            environment.join("\n")
        ));
    }
    println!("ok    panel     {}", png_path.display());
    Ok(())
}

/// `eseq sequencer check MODULE`: find the module on the load path, check
/// its header and `def-kind`s, then load it in the app's own runtime
/// (`metal_seq capture`), create an instance of each kind and draw its tab.
/// The tick is not run; the report says so.
fn check_sequencer(module: &str, render: bool, eval: Option<&str>) -> Result<(), String> {
    if !eseqlisp::modules::is_valid_module_name(module) || module.contains('"') {
        return Err(format!("`{module}` is not a module name (dotted, like `my.pulse`)"));
    }
    let paths = sequencer::app_paths::app_paths();
    let (roots, _) = paths.module_load_roots();
    let path = resolve_module_file(&roots, module).ok_or_else(|| {
        let searched: Vec<String> = roots.iter().map(|root| root.path.display().to_string()).collect();
        format!(
            "FAIL {module} (module)\nno file for `{module}`; a Local module `a.b` is \
             `a/b.lisp` under {}. Searched:\n  {}",
            paths.local_modules_dir().display(),
            searched.join("\n  ")
        )
    })?;
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    match declared_module(&source) {
        Some(name) if name == module => {}
        Some(name) => {
            return Err(format!(
                "FAIL {module} (module)\n{} declares (module {name}); it must declare (module {module})",
                path.display()
            ))
        }
        None => {
            return Err(format!(
                "FAIL {module} (module)\n{} must begin with (module {module})",
                path.display()
            ))
        }
    }
    println!("ok    module    {}", path.display());
    let checked = check_brackets_with_imports(&roots, paths, module, &path, &source)?;
    println!("ok    parse     {checked} file(s), brackets balanced");
    let kinds = def_kind_names(&source);
    if kinds.is_empty() {
        return Err(format!(
            "FAIL {module} (def-kind)\nno (def-kind NAME …) in {}; the check needs the module \
             that declares the kind (usually the one with the panel)",
            path.display()
        ));
    }
    println!("ok    def-kind  {}", kinds.join(", "));
    if render {
        let package = sequencer::lisp_host::package_name_for_module(module);
        for kind in &kinds {
            let id = sequencer::lisp_host::kind_id(package.as_deref(), Some(module), kind);
            render_instance(module, &path, kind, &id, eval)?;
        }
    }
    println!("--    tick      not run by the check: press Play in eseq and listen");
    println!("PASS {module}");
    Ok(())
}

/// The file a module loads from: the first load root (Local, installed
/// packages, factory) holding one of its candidate paths. A package root
/// only resolves modules in its own namespace, prefix stripped.
fn resolve_module_file(roots: &[eseqlisp::ModuleLoadRoot], module: &str) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let relative = match &root.module_prefix {
            Some(prefix) => module.strip_prefix(prefix.as_str())?.strip_prefix('.')?,
            None => module,
        };
        eseqlisp::modules::module_relative_file_candidates(relative)
            .into_iter()
            .map(|candidate| root.path.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

/// Source with `;` comments dropped and string contents blanked, so neither
/// can look like a form (eseqlisp strings have no escapes). String state
/// carries across lines (strings may span them); newlines are always kept so
/// reported line numbers stay correct.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut in_comment = false;
    for ch in source.chars() {
        match ch {
            '\n' => {
                in_comment = false;
                out.push('\n');
            }
            _ if in_comment => {}
            '"' => {
                in_string = !in_string;
                out.push('"');
            }
            ';' if !in_string => in_comment = true,
            _ if in_string => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Bracket-check the module and every Local or installed module it imports,
/// transitively (factory modules are eseq's own and skipped). Returns how
/// many files were checked.
fn check_brackets_with_imports(
    roots: &[eseqlisp::ModuleLoadRoot],
    paths: &sequencer::app_paths::AppPaths,
    module: &str,
    path: &Path,
    source: &str,
) -> Result<usize, String> {
    let user_roots = [paths.local_modules_dir(), paths.packages_dir()];
    let mut pending = vec![(module.to_string(), path.to_path_buf(), source.to_string())];
    let mut seen = vec![module.to_string()];
    let mut checked = 0;
    while let Some((name, file, text)) = pending.pop() {
        if let Some(problem) = bracket_problem(&text) {
            return Err(format!("FAIL {module} (parse)\n{}:{problem}", file.display()));
        }
        checked += 1;
        for import in imported_modules(&text) {
            if seen.contains(&import) {
                continue;
            }
            seen.push(import.clone());
            let Some(import_file) = resolve_module_file(roots, &import) else { continue };
            if !user_roots.iter().any(|root| import_file.starts_with(root)) {
                continue;
            }
            let import_text = std::fs::read_to_string(&import_file)
                .map_err(|error| format!("cannot read {}: {error}", import_file.display()))?;
            pending.push((import, import_file, import_text));
        }
        let _ = name;
    }
    Ok(checked)
}

/// `LINE: reason` for the first unbalanced bracket, or None.
fn bracket_problem(source: &str) -> Option<String> {
    let mut open: Vec<(char, usize)> = Vec::new();
    for (index, line) in code_only(source).lines().enumerate() {
        let line_no = index + 1;
        for ch in line.chars() {
            match ch {
                '(' | '[' => open.push((ch, line_no)),
                ')' | ']' => {
                    let want = if ch == ')' { '(' } else { '[' };
                    match open.pop() {
                        Some((got, _)) if got == want => {}
                        Some((got, at)) => {
                            return Some(format!(
                                "{line_no}: `{ch}` closes the `{got}` opened on line {at}"
                            ))
                        }
                        None => return Some(format!("{line_no}: `{ch}` has nothing to close")),
                    }
                }
                _ => {}
            }
        }
    }
    open.pop().map(|(ch, at)| format!("{at}: this `{ch}` is never closed"))
}

/// Module names in `(import NAME …)` forms.
fn imported_modules(source: &str) -> Vec<String> {
    let code = code_only(source);
    let mut names = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find("(import") {
        rest = &rest[at + "(import".len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|ch| !ch.is_whitespace() && *ch != ')' && *ch != '(')
            .collect();
        if eseqlisp::modules::is_valid_module_name(&name) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The name in a leading `(module NAME)` form, if the file starts with one.
fn declared_module(source: &str) -> Option<String> {
    let code = code_only(source);
    let rest = code.trim_start().strip_prefix('(')?.trim_start().strip_prefix("module")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != ')')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Every `(def-kind NAME` in the source, in order.
fn def_kind_names(source: &str) -> Vec<String> {
    let code = code_only(source);
    let mut names = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find("(def-kind") {
        rest = &rest[at + "(def-kind".len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|ch| !ch.is_whitespace() && *ch != ')' && *ch != '(')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Load the module in the app's own runtime, create instance 1 of the kind
/// and draw its tab. `eval` runs after the project syncs, with the instance
/// reachable as `(instance-ref 1)`: fill its document with an example, or pin
/// a preview playhead.
fn render_instance(
    module: &str,
    path: &Path,
    kind: &str,
    kind_id: &str,
    eval: Option<&str>,
) -> Result<(), String> {
    let metal_seq = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("metal_seq")))
        .filter(|path| path.exists());
    let Some(metal_seq) = metal_seq else {
        println!("--    panel     not rendered: metal_seq not found next to eseq");
        return Ok(());
    };
    let work_dir = std::env::temp_dir().join("eseq-check");
    std::fs::create_dir_all(&work_dir)
        .map_err(|error| format!("cannot create {}: {error}", work_dir.display()))?;
    let stem: String = format!("{module}-{kind}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let script_path = work_dir.join(format!("{stem}.lisp"));
    let png_path = work_dir.join(format!("{stem}.png"));
    let mut script = String::from(
        "(capture-project\n  (track :sampler :name \"Kick\")\n  (track :sampler :name \"Snare\")\n  \
         (track :sampler :name \"Hat\")\n  (track :sampler :name \"Perc\"))\n",
    );
    script.push_str(&format!("(import {module})\n"));
    script.push_str(&format!("(host-command \"instance-create\" (dict :kind \"{kind_id}\"))\n"));
    if let Some(form) = eval {
        script.push_str(&format!("(def capture-after-sync () {form})\n"));
    }
    std::fs::write(&script_path, script)
        .map_err(|error| format!("cannot write {}: {error}", script_path.display()))?;
    let buffer = format!("*{kind} · {kind} 1*");
    let output = std::process::Command::new(&metal_seq)
        .arg("capture")
        .arg("--script")
        .arg(&script_path)
        .args(["--buffer", &buffer, "--hide-status"])
        .args(["--width", "1800", "--height", "900"])
        .arg("--out")
        .arg(&png_path)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", metal_seq.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let file = path.display().to_string();
    let own: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.contains("is not exported by"))
        .filter(|line| {
            line.contains(&file)
                || line.contains("[lisp-error]")
                || line.contains(module)
                || line.contains("capture-after-sync failed")
        })
        .collect();
    if !own.is_empty() || !output.status.success() {
        let mut detail = if own.is_empty() {
            stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n")
        } else {
            own.join("\n")
        };
        if stderr.contains("does not exist") {
            detail.push_str(&format!(
                "\nthe `{kind}` tab did not open: the module failed to load, `def-kind {kind}` \
                 has no :view, or creating the instance failed"
            ));
        }
        return Err(format!("FAIL {module} (panel render, kind {kind})\n{detail}"));
    }
    println!("ok    panel     {kind}: {}", png_path.display());
    Ok(())
}

fn index_package(path: PathBuf) -> Result<(), String> {
    let lines = sequencer::sample_manifest::index_package(&path)?;
    let count = lines
        .iter()
        .filter(|line| {
            matches!(
                line,
                sequencer::sample_manifest::SampleManifestLine::Sample(_)
            )
        })
        .count();
    println!(
        "indexed {count} sample(s) into {}",
        path.join("samples.jsonl").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bracket_problem, declared_module, def_kind_names, imported_modules};

    #[test]
    fn sequencer_check_reads_the_module_header_and_kind_names() {
        let source = ";; (def-kind commented-out)\n(module my.pulse)\n(import my.pulse-core)\n\
                      (def x \"(def-kind in-a-string\")\n(def-kind pulse\n  :view p)\n\
                      (def-kind other :view q) ; (def-kind trailing)\n(def-kinds nope)\n";
        assert_eq!(declared_module(source).as_deref(), Some("my.pulse"));
        assert_eq!(def_kind_names(source), vec!["pulse".to_string(), "other".to_string()]);
        assert_eq!(declared_module("(def x 1)\n(module late)"), None);
        assert_eq!(declared_module("(modules x)"), None);
        assert_eq!(imported_modules(source), vec!["my.pulse-core".to_string()]);
    }

    #[test]
    fn sequencer_check_points_at_the_unbalanced_bracket() {
        assert_eq!(bracket_problem("(a (b \")\") ; )\n c)"), None);
        assert_eq!(
            bracket_problem("(def a 1)\n(def b (+ 1\n  2)\n(def c 3)").as_deref(),
            Some("2: this `(` is never closed")
        );
        assert_eq!(bracket_problem("(a))").as_deref(), Some("1: `)` has nothing to close"));
        assert_eq!(
            bracket_problem("(each xs |i|\n  [a)").as_deref(),
            Some("2: `)` closes the `[` opened on line 2")
        );
    }

    #[test]
    fn sequencer_check_tracks_strings_across_lines() {
        assert_eq!(bracket_problem("(def doc \"first\nhas (paren; and ) \")\n(def b 1)"), None);
        assert_eq!(
            def_kind_names("(module m)\n(def d \"x\n(def-kind fake\")\n(def-kind real :view v)"),
            vec!["real".to_string()]
        );
        assert_eq!(
            bracket_problem("(def doc \"a\nb\")\n(def c (+ 1\n 2)").as_deref(),
            Some("3: this `(` is never closed")
        );
    }
}
