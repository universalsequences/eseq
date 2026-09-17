//! Export user-tier instruments and effects as a distributable package.
//!
//! The writer mirrors the factory layout inside the package (content-tiers
//! spec §4.0): each picked instrument folder lands under `instruments/`, each
//! effect under `effects/`, with a `manifest.json` naming the pack. Sources
//! are copied verbatim except for one rewrite: `(use-defmacro …)` forms are
//! inlined from the local macro library, so the pack compiles on a machine
//! that never saw those macros. The resulting directory is what
//! [`crate::package_install::stage_package_from_path`] accepts, so
//! export → import is a pure round trip.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::app_paths::{AppPaths, ContentTier};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportKind {
    Instrument,
    Effect,
}

impl ExportKind {
    fn content_dir(self) -> &'static str {
        match self {
            Self::Instrument => "instruments",
            Self::Effect => "effects",
        }
    }
}

/// One user-tier item offered for export: its bare logical path inside the
/// user tier (`kits/808`), which is also its path inside the package.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ExportCandidate {
    pub kind: ExportKind,
    pub logical: String,
    /// `<logical>/dsp.lisp` for folder layouts, `<logical>.lisp` for flat.
    pub source: PathBuf,
}

/// Every instrument and effect in the user tier, sorted by kind then name.
pub fn export_candidates(paths: &AppPaths) -> Vec<ExportCandidate> {
    fn collect(kind: ExportKind, dir: &Path, root: &Path, out: &mut Vec<ExportCandidate>) {
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
                if path.join("dsp.lisp").is_file() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(ExportCandidate {
                            kind,
                            logical: rel.to_string_lossy().replace('\\', "/"),
                            source: path.join("dsp.lisp"),
                        });
                    }
                } else {
                    collect(kind, &path, root, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "lisp")
                && !matches!(path.file_stem().and_then(|s| s.to_str()), Some("dsp" | "ui"))
            {
                if let Ok(rel) = path.with_extension("").strip_prefix(root) {
                    out.push(ExportCandidate {
                        kind,
                        logical: rel.to_string_lossy().replace('\\', "/"),
                        source: path.clone(),
                    });
                }
            }
        }
    }
    let mut out = Vec::new();
    let instruments = paths.user_instruments_dir();
    collect(ExportKind::Instrument, &instruments, &instruments, &mut out);
    let effects = paths.user_effects_dir();
    collect(ExportKind::Effect, &effects, &effects, &mut out);
    out.sort_by(|a, b| (a.kind as u8, a.logical.to_lowercase()).cmp(&(b.kind as u8, b.logical.to_lowercase())));
    out
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportRequest {
    /// `author/name`.
    pub identity: String,
    pub version: String,
    pub items: Vec<(ExportKind, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// The package directory (`<out>/<author.name>/`).
    pub package_dir: PathBuf,
    /// The zipped package when archiving was requested and succeeded.
    pub archive: Option<PathBuf>,
    pub instruments: usize,
    pub effects: usize,
    /// Things the author should look at before sharing: absolute paths in
    /// a source, a macro that could not be inlined, a missing preset bank.
    pub warnings: Vec<String>,
}

/// Files inside an instrument/effect folder that never belong in a pack:
/// compile caches, audition previews, editor and VCS droppings.
fn is_export_junk(path: &Path, is_dir: bool) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with('.') {
        return true;
    }
    if is_dir {
        return name.ends_with("-cache") || name == "__pycache__" || name == "target";
    }
    name.starts_with("preview-") && name.ends_with(".wav")
        || name.ends_with(".dylib")
        || name.ends_with(".so")
        || name.ends_with(".o")
}

/// The archive name a pack is offered under: `author.name-version.eseqpack`.
pub fn archive_file_name(identity: &str, version: &str) -> String {
    format!("{}-{version}.eseqpack", identity.replace('/', "."))
}

/// Write the package directory under `out_dir` (creating
/// `<out_dir>/<author.name>/`) and, when `archive` is set, zip it beside as
/// [`archive_file_name`]. Refuses to overwrite an existing package directory
/// or archive.
pub fn export_package(
    paths: &AppPaths,
    request: &ExportRequest,
    out_dir: &Path,
    archive: bool,
) -> Result<ExportReport, String> {
    let prefix = eseqlisp::package::validate_package_name(&request.identity)?;
    let version = request.version.trim();
    if version.is_empty() {
        return Err("version must not be empty".into());
    }
    if request.items.is_empty() {
        return Err("pick at least one instrument or effect".into());
    }
    let candidates = export_candidates(paths);
    let package_dir = out_dir.join(&prefix);
    if package_dir.exists() {
        return Err(format!("{} already exists", package_dir.display()));
    }
    let archive_path = archive.then(|| out_dir.join(archive_file_name(&request.identity, version)));
    if let Some(archive_path) = &archive_path {
        if archive_path.exists() {
            return Err(format!("{} already exists", archive_path.display()));
        }
    }

    let mut report = ExportReport {
        package_dir: package_dir.clone(),
        ..Default::default()
    };
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&package_dir)
            .map_err(|error| format!("failed to create {}: {error}", package_dir.display()))?;
        for (kind, logical) in &request.items {
            let logical = logical.trim_end_matches('/');
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.kind == *kind && candidate.logical == logical)
                .ok_or_else(|| {
                    format!(
                        "{} '{logical}' is not in your library (only user-tier content exports; fork factory content first)",
                        match kind {
                            ExportKind::Instrument => "instrument",
                            ExportKind::Effect => "effect",
                        }
                    )
                })?;
            export_candidate(paths, candidate, &package_dir, &mut report)?;
            match kind {
                ExportKind::Instrument => report.instruments += 1,
                ExportKind::Effect => report.effects += 1,
            }
        }
        let manifest = serde_json::json!({
            "name": request.identity,
            "version": version,
        });
        std::fs::write(
            package_dir.join("manifest.json"),
            format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap()),
        )
        .map_err(|error| format!("failed to write manifest.json: {error}"))?;
        // The pack must be exactly what import accepts.
        eseqlisp::package::InstalledPackage::load(&package_dir).map_err(|error| error.to_string())?;
        if let Some(archive_path) = &archive_path {
            zip_directory(&package_dir, archive_path)?;
            report.archive = Some(archive_path.clone());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&package_dir);
        if let Some(archive_path) = &archive_path {
            let _ = std::fs::remove_file(archive_path);
        }
        return Err(error);
    }
    Ok(report)
}

fn export_candidate(
    paths: &AppPaths,
    candidate: &ExportCandidate,
    package_dir: &Path,
    report: &mut ExportReport,
) -> Result<(), String> {
    let target_root = package_dir.join(candidate.kind.content_dir());
    let folder_layout = candidate.source.file_name().and_then(|n| n.to_str()) == Some("dsp.lisp");
    let label = format!("{:?} {}", candidate.kind, candidate.logical).to_lowercase();
    if folder_layout {
        let source_dir = candidate.source.parent().expect("dsp.lisp has a folder");
        let target_dir = target_root.join(&candidate.logical);
        copy_dir_filtered(source_dir, &target_dir)?;
        materialize_dsp(&target_dir.join("dsp.lisp"), &label, report)?;
        // The preset bank sits beside the folder, not inside it.
        let bank = source_dir.with_extension("presets");
        if bank.is_file() {
            std::fs::copy(&bank, target_dir.with_extension("presets"))
                .map_err(|error| format!("failed to copy {}: {error}", bank.display()))?;
        }
    } else {
        let target = target_root.join(format!("{}.lisp", candidate.logical));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        std::fs::copy(&candidate.source, &target)
            .map_err(|error| format!("failed to copy {}: {error}", candidate.source.display()))?;
        materialize_dsp(&target, &label, report)?;
        for sidecar in ["presets", "instrument.json", "layout.json"] {
            let file = candidate.source.with_extension(sidecar);
            if file.is_file() {
                std::fs::copy(&file, target.with_extension(sidecar))
                    .map_err(|error| format!("failed to copy {}: {error}", file.display()))?;
            }
        }
    }
    // Effects keep their user-tier `.presets`-less shape; instruments with a
    // factory-shipped bank plus a user overlay would have been forked first,
    // so nothing merges here.
    let _ = paths;
    Ok(())
}

/// Inline library macros and flag references that will not travel.
fn materialize_dsp(dsp: &Path, label: &str, report: &mut ExportReport) -> Result<(), String> {
    let source = std::fs::read_to_string(dsp)
        .map_err(|error| format!("failed to read {}: {error}", dsp.display()))?;
    let materialized = if source.contains("use-defmacro") {
        match eseqlisp::defmacro_library::materialize_with_default_library(&source) {
            Ok(materialized) => materialized,
            Err(error) => {
                report.warnings.push(format!(
                    "{label}: could not inline its library macros ({error}); the pack needs the same macro library to compile"
                ));
                source.clone()
            }
        }
    } else {
        source.clone()
    };
    for line in materialized.lines() {
        if let Some(start) = line.find("\"/") {
            let rest = &line[start + 1..];
            let end = rest.find('"').unwrap_or(rest.len());
            let literal = &rest[..end];
            if Path::new(literal).is_absolute() && literal.len() > 1 {
                report.warnings.push(format!(
                    "{label}: references the absolute path {literal}, which will not exist on another machine"
                ));
            }
        }
    }
    if materialized != source {
        std::fs::write(dsp, materialized)
            .map_err(|error| format!("failed to write {}: {error}", dsp.display()))?;
    }
    Ok(())
}

fn copy_dir_filtered(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read {}: {error}", source.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        if file_type.is_symlink() || is_export_junk(&path, file_type.is_dir()) {
            continue;
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_filtered(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|error| format!("failed to copy {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// Zip `dir` so the archive wraps the directory itself (what
/// `stage_package_from_path` unwraps): `ditto -c -k --keepParent` on macOS,
/// `zip -r` elsewhere.
fn zip_directory(dir: &Path, archive: &Path) -> Result<(), String> {
    let parent = dir.parent().ok_or("package directory has no parent")?;
    let name = dir.file_name().and_then(|n| n.to_str()).ok_or("package directory has no name")?;
    let attempts: Vec<(&str, Vec<String>)> = {
        let ditto = (
            "ditto",
            vec![
                "-c".into(),
                "-k".into(),
                "--keepParent".into(),
                dir.display().to_string(),
                archive.display().to_string(),
            ],
        );
        let zip = (
            "zip",
            vec!["-q".into(), "-r".into(), archive.display().to_string(), name.to_string()],
        );
        if cfg!(target_os = "macos") { vec![ditto, zip] } else { vec![zip, ditto] }
    };
    let mut last_error = String::new();
    for (program, args) in attempts {
        let mut command = Command::new(program);
        command.args(&args);
        if program == "zip" {
            command.current_dir(parent);
        }
        match command.output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                last_error = format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim())
            }
            Err(error) => last_error = format!("failed to launch {program}: {error}"),
        }
    }
    Err(format!("could not create {}: {last_error}", archive.display()))
}

/// The tier-qualified id an exported item will have once the pack is
/// installed, for status lines and the round-trip test.
pub fn exported_id(identity: &str, logical: &str) -> String {
    ContentTier::Package(identity.replace('/', ".")).qualify(logical.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> (AppPaths, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-export-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let paths = AppPaths::dev(root.join("crates/sequencer"), root.clone(), root.join("config"));
        paths.ensure_user_tier().unwrap();
        (paths, root)
    }

    #[test]
    fn export_round_trips_through_import_with_junk_dropped_and_presets_carried() {
        let (paths, root) = temp_paths("roundtrip");
        // A folder instrument with a preset bank beside it and junk inside.
        let kick = paths.user_instruments_dir().join("kits/kick");
        std::fs::create_dir_all(kick.join("waves")).unwrap();
        std::fs::create_dir_all(kick.join("kick-cache")).unwrap();
        std::fs::write(kick.join("dsp.lisp"), "(param tone @min 0 @max 1)\n(out (sine 60) 1)").unwrap();
        std::fs::write(kick.join("ui.lisp"), "(def ui 1)").unwrap();
        std::fs::write(kick.join("instrument.json"), r#"{"version":1,"run_mode":"instrument"}"#).unwrap();
        std::fs::write(kick.join("waves/bank.json"), "{}").unwrap();
        std::fs::write(kick.join("preview-defaults.wav"), b"RIFF").unwrap();
        std::fs::write(kick.join("kick-cache/x.dylib"), b"").unwrap();
        std::fs::write(kick.with_extension("presets"), r#"{"version":1,"engine_name":"user:kits/kick","source_file":"x","presets":[]}"#).unwrap();
        // A flat effect and a folder effect.
        std::fs::write(paths.user_effects_dir().join("gain.lisp"), "(out (* (in 1) 0.5) 1)").unwrap();
        let comp = paths.user_effects_dir().join("comp");
        std::fs::create_dir_all(&comp).unwrap();
        std::fs::write(comp.join("dsp.lisp"), "(out (in 1) 1)").unwrap();
        // A factory instrument must not be offered.
        std::fs::create_dir_all(paths.instruments_dir().join("shipped")).unwrap();
        std::fs::write(paths.instruments_dir().join("shipped/dsp.lisp"), "(out 0)").unwrap();

        let candidates = export_candidates(&paths);
        assert_eq!(
            candidates.iter().map(|c| (c.kind, c.logical.as_str())).collect::<Vec<_>>(),
            vec![
                (ExportKind::Instrument, "kits/kick"),
                (ExportKind::Effect, "comp"),
                (ExportKind::Effect, "gain"),
            ]
        );

        let out = root.join("export");
        std::fs::create_dir_all(&out).unwrap();
        let request = ExportRequest {
            identity: "alec/drums".into(),
            version: "1.0".into(),
            items: vec![
                (ExportKind::Instrument, "kits/kick/".into()),
                (ExportKind::Effect, "comp".into()),
                (ExportKind::Effect, "gain".into()),
            ],
        };
        let report = export_package(&paths, &request, &out, true).unwrap();
        assert_eq!(report.package_dir, out.join("alec.drums"));
        assert_eq!(report.archive, Some(out.join("alec.drums-1.0.eseqpack")));
        assert_eq!((report.instruments, report.effects), (1, 2));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let pack = &report.package_dir;
        assert!(pack.join("instruments/kits/kick/dsp.lisp").is_file());
        assert!(pack.join("instruments/kits/kick/ui.lisp").is_file());
        assert!(pack.join("instruments/kits/kick/waves/bank.json").is_file());
        assert!(pack.join("instruments/kits/kick.presets").is_file(), "the bank travels beside the folder");
        assert!(!pack.join("instruments/kits/kick/preview-defaults.wav").exists());
        assert!(!pack.join("instruments/kits/kick/kick-cache").exists());
        assert!(pack.join("effects/comp/dsp.lisp").is_file());
        assert!(pack.join("effects/gain.lisp").is_file());
        let manifest = std::fs::read_to_string(pack.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"alec/drums\""));

        // Exporting again refuses to clobber.
        let error = export_package(&paths, &request, &out, false).unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        // A factory-only or unknown item is rejected, leaving nothing behind.
        let bad = ExportRequest {
            identity: "alec/other".into(),
            version: "1".into(),
            items: vec![(ExportKind::Instrument, "shipped".into())],
        };
        let error = export_package(&paths, &bad, &out, false).unwrap_err();
        assert!(error.contains("not in your library"), "{error}");
        assert!(!out.join("alec.other").exists());

        // Round trip: the archive imports, and the instrument resolves under
        // its package id — never as the user-tier original.
        let staged = crate::package_install::stage_package_from_path(
            report.archive.as_ref().unwrap(),
            &paths.packages_dir(),
        )
        .unwrap();
        assert_eq!(staged.summary.instruments, 1);
        assert_eq!(staged.summary.effects, 2);
        let installed = crate::package_install::publish_staged_package(staged, false).unwrap();
        assert_eq!(installed.path, paths.packages_dir().join("alec.drums"));
        let id = exported_id("alec/drums", "kits/kick/");
        assert_eq!(id, "pkg:alec.drums/kits/kick");
        let resolved = crate::lisp_host::instrument_source_path_with_paths_for_tests(&paths, &id).unwrap();
        assert_eq!(resolved, installed.path.join("instruments/kits/kick/dsp.lisp"));
        assert!(paths
            .effect_roots()
            .iter()
            .any(|root| root.tier == ContentTier::Package("alec.drums".into())));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_inlines_library_macros_and_flags_absolute_paths() {
        let (paths, root) = temp_paths("macros");
        let fx = paths.user_effects_dir().join("weird");
        std::fs::create_dir_all(&fx).unwrap();
        std::fs::write(
            fx.join("dsp.lisp"),
            "(use-defmacro no-such-macro-for-export-test)\n(def ir \"/Users/someone/impulse.wav\")\n(out (in 1) 1)",
        )
        .unwrap();
        let out = root.join("export");
        std::fs::create_dir_all(&out).unwrap();
        let report = export_package(
            &paths,
            &ExportRequest {
                identity: "alec/fx".into(),
                version: "0.1".into(),
                items: vec![(ExportKind::Effect, "weird".into())],
            },
            &out,
            false,
        )
        .unwrap();
        assert!(report.archive.is_none());
        assert!(
            report.warnings.iter().any(|w| w.contains("absolute path /Users/someone/impulse.wav")),
            "{:?}",
            report.warnings
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("library macros")),
            "an unknown macro is reported, not silently shipped: {:?}",
            report.warnings
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
