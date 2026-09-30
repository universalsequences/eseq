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
