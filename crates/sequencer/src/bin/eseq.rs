use std::path::PathBuf;

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("eseq: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.as_slice() {
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
                let kind = match kind {
                    "instrument" => ExportKind::Instrument,
                    "effect" => ExportKind::Effect,
                    other => return Err(format!("unknown item kind `{other}`")),
                };
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
                "exported {} ({} instrument(s), {} effect(s))",
                report.archive.as_deref().unwrap_or(&report.package_dir).display(),
                report.instruments,
                report.effects
            );
            for warning in report.warnings {
                eprintln!("eseq: warning: {warning}");
            }
            Ok(())
        }
        _ => Err(
            "usage: eseq package index [PACKAGE_DIR]\n       eseq package install AUTHOR/NAME GIT_URL\n       eseq package import PATH_OR_ARCHIVE\n       eseq package export AUTHOR/NAME VERSION OUT_DIR (instrument:<name>|effect:<name>)..."
                .to_string(),
        ),
    }
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
