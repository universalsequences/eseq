//! The authoring kit for coding agents (eseq-63j4). Users installing from the
//! DMG have no checkout to read, so the app writes a guide into the user data
//! root: open a terminal there, start an agent (Claude Code reads `CLAUDE.md`,
//! Codex and others read `AGENTS.md`), and ask for an instrument. The guide
//! points at the rules in [`AppPaths::authoring_dir`] (shared with in-app
//! Agent Mode's prompts), the factory examples, and `eseq instrument check`.
//!
//! Every generated file carries [`GENERATED_MARKER`]. A file with the marker is
//! rewritten on each seed, which keeps paths and rules current across app
//! updates; a user who deletes the marker line owns the file from then on.

use std::io;
use std::path::{Path, PathBuf};

use crate::app_paths::AppPaths;

pub const GENERATED_MARKER: &str = "<!-- eseq:generated";

const AGENTS_TEMPLATE: &str = include_str!("AGENTS.md.in");
const SKILL_TEMPLATE: &str = include_str!("SKILL.md.in");
const CLAUDE_MD: &str = "<!-- eseq:generated. ESeq rewrites this file each time it starts. Delete this line to keep your own edits. -->\n@AGENTS.md\n";

/// Where the kit's files land inside the seeded folder.
pub const AGENTS_FILE: &str = "AGENTS.md";
pub const CLAUDE_FILE: &str = "CLAUDE.md";
pub const SKILL_FILE: &str = ".claude/skills/eseq-authoring/SKILL.md";

#[derive(Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    Written(PathBuf),
    /// The file exists without the marker: the user took it over.
    KeptUserFile(PathBuf),
}

/// The `eseq` command-line tool next to the running executable (the app and
/// the CLI share `Contents/MacOS`, or one Cargo target directory).
pub fn eseq_cli_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("eseq")))
        .unwrap_or_else(|| PathBuf::from("eseq"))
}

fn render(template: &str, paths: &AppPaths, eseq_cli: &Path) -> String {
    let path = |path: PathBuf| path.display().to_string();
    template
        .replace("{{USER_ROOT}}", &path(paths.user_data_root()))
        .replace("{{USER_INSTRUMENTS}}", &path(paths.user_instruments_dir()))
        .replace("{{USER_EFFECTS}}", &path(paths.user_effects_dir()))
        .replace("{{FACTORY_INSTRUMENTS}}", &path(paths.instruments_dir()))
        .replace("{{FACTORY_EFFECTS}}", &path(paths.effects_dir()))
        .replace("{{AUTHORING}}", &path(paths.authoring_dir()))
        .replace("{{LOCAL_PACKAGES}}", &path(paths.local_modules_dir()))
        .replace("{{FACTORY_PACKAGES}}", &path(paths.factory_packages_dir()))
        .replace("{{DGENLISP_REFERENCE}}", &path(paths.dgenlisp_reference_dir()))
        .replace("{{ESEQ}}", &eseq_cli.display().to_string())
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
}

pub fn agents_md(paths: &AppPaths, eseq_cli: &Path) -> String {
    render(AGENTS_TEMPLATE, paths, eseq_cli)
}

/// A Claude Code skill that points at the seeded guide, for installing under
/// `~/.claude/skills/` so it applies from any directory.
pub fn skill_md(paths: &AppPaths, eseq_cli: &Path) -> String {
    render(SKILL_TEMPLATE, paths, eseq_cli)
}

/// Write the kit into `root` (normally [`AppPaths::user_data_root`]).
pub fn seed(paths: &AppPaths, root: &Path, eseq_cli: &Path) -> io::Result<Vec<SeedOutcome>> {
    let files = [
        (AGENTS_FILE, agents_md(paths, eseq_cli)),
        (CLAUDE_FILE, CLAUDE_MD.to_string()),
        (SKILL_FILE, skill_md(paths, eseq_cli)),
    ];
    let mut outcomes = Vec::new();
    for (relative, contents) in files {
        let target = root.join(relative);
        match std::fs::read_to_string(&target) {
            Ok(existing) if !existing.contains(GENERATED_MARKER) => {
                outcomes.push(SeedOutcome::KeptUserFile(target));
                continue;
            }
            Ok(existing) if existing == contents => {
                outcomes.push(SeedOutcome::Written(target));
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, contents)?;
        outcomes.push(SeedOutcome::Written(target));
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eseq-authoring-kit-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn rendered_guide_has_no_unfilled_placeholders_and_names_the_check_command() {
        let paths = crate::app_paths::app_paths();
        let cli = Path::new("/Applications/ESeq.app/Contents/MacOS/eseq");
        for text in [agents_md(paths, cli), skill_md(paths, cli)] {
            assert!(!text.contains("{{"), "unfilled placeholder in:\n{text}");
            assert!(text.contains("\"/Applications/ESeq.app/Contents/MacOS/eseq\" instrument check"));
            assert!(text.contains(&paths.user_instruments_dir().display().to_string()));
            assert!(text.contains("\"/Applications/ESeq.app/Contents/MacOS/eseq\" sequencer check"));
            assert!(text.contains(&paths.local_modules_dir().display().to_string()));
        }
    }

    #[test]
    fn release_guide_points_inside_the_bundle_and_application_support() {
        let bundle = Path::new("/Applications/ESeq.app/Contents");
        let support = Path::new("/Users/test/Library/Application Support/com.universalsequences.eseq");
        let paths = AppPaths::release(
            bundle.join("MacOS"),
            bundle.join("Resources"),
            support.to_path_buf(),
            PathBuf::from("/Users/test/Library/Caches/com.universalsequences.eseq"),
            PathBuf::from("/Users/test/.eseq.d"),
        );
        let text = agents_md(&paths, &bundle.join("MacOS/eseq"));
        for expected in [
            "/Applications/ESeq.app/Contents/Resources/authoring/instrument-reference.md",
            "/Applications/ESeq.app/Contents/Resources/authoring/DGenLispReadme.md",
            "/Applications/ESeq.app/Contents/Resources/instruments/Synths/Revsynt/dsp.lisp",
            "/Users/test/Library/Application Support/com.universalsequences.eseq/instruments",
        ] {
            assert!(text.contains(expected), "missing {expected} in:\n{text}");
        }
    }

    /// The dgenlisp code block in `reference` that contains `needle`, with
    /// the Markdown list indentation removed.
    fn recipe_block(reference: &str, needle: &str) -> String {
        let text = std::fs::read_to_string(crate::app_paths::app_paths().authoring_dir().join(reference))
            .unwrap();
        text.split("```dgenlisp")
            .skip(1)
            .map(|block| block.split("```").next().unwrap())
            .find(|block| block.contains(needle))
            .unwrap_or_else(|| panic!("no recipe containing {needle} in {reference}"))
            .lines()
            .map(|line| line.strip_prefix("  ").unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reference_recipes_compile_and_make_sound() {
        use crate::agent::verify::{verify_sources, ArtifactKind, VerifyMode};

        let drum = format!(
            "(def gate (in 1 @name gate))\n(def pitch (in 2 @name pitch))\n\
             (def velocity (in 3 @name velocity))\n(def trigger (in 4 @name trigger))\n\
             (def clock (in 5 @name clock))\n\
             {}\n(out (* (sin (* twopi (phasor 55))) amp_env hit_velocity 0.5) 1 @name audio)\n",
            recipe_block("instrument-reference.md", "defmacro onset-of")
        );
        let report = verify_sources(ArtifactKind::Instrument, VerifyMode::Library, &drum, None, 44_100, None)
            .unwrap_or_else(|failure| panic!("one-shot recipe: {failure}\n{drum}"));
        assert!(!report.audition.silent);

        let echo = format!(
            "(def in_l (in 1 @name left))\n(def in_r (in 2 @name right))\n\
             (param time_ms @default 120 @min 10 @max 1000 @unit ms)\n\
             {}\n(def t (* time_ms (/ samplerate 1000)))\n\
             (out (+ in_l (echo in_l t 0.5 3000)) 1 @name left)\n\
             (out (+ in_r (echo in_r t 0.5 3000)) 2 @name right)\n",
            recipe_block("effect-reference.md", "defmacro echo")
        );
        verify_sources(ArtifactKind::Effect, VerifyMode::Library, &echo, None, 44_100, None)
            .unwrap_or_else(|failure| panic!("feedback-delay recipe: {failure}\n{echo}"));
    }

    #[test]
    fn referenced_kit_files_exist() {
        let paths = crate::app_paths::app_paths();
        for file in ["instrument-reference.md", "effect-reference.md", "sequencer-reference.md"] {
            assert!(paths.authoring_dir().join(file).is_file(), "missing {file}");
        }
        for file in ["DGenLispReadme.md", "dgenlisp-operators.json"] {
            assert!(paths.dgenlisp_reference_dir().join(file).is_file(), "missing {file}");
        }
        for example in [
            "Synths/Revsynt",
            "Synths/Vox",
            "Drums/Digi Snare",
            "Drums/Boom Bap Kick",
            "Physical Models/PM Flute",
        ] {
            assert!(paths.instruments_dir().join(example).join("dsp.lisp").is_file(), "{example}");
        }
        for example in ["stereo-tremolo", "dimension-d-chorus", "lexilush"] {
            assert!(paths.effects_dir().join(example).join("ui.lisp").is_file(), "{example}");
        }
        for example in ["alez.jaki/src/kind.lisp", "alez.jaki/src/doc.lisp"] {
            assert!(paths.factory_packages_dir().join(example).is_file(), "{example}");
        }
    }

    #[test]
    fn seed_refreshes_generated_files_and_keeps_user_owned_ones() {
        let paths = crate::app_paths::app_paths();
        let root = temp_root("seed");
        let cli = Path::new("/opt/eseq");
        let first = seed(paths, &root, cli).unwrap();
        assert!(first.iter().all(|outcome| matches!(outcome, SeedOutcome::Written(_))));
        assert_eq!(
            std::fs::read_to_string(root.join(CLAUDE_FILE)).unwrap().lines().last(),
            Some("@AGENTS.md")
        );

        // A stale generated file is refreshed; a file without the marker is
        // the user's.
        std::fs::write(root.join(AGENTS_FILE), format!("{GENERATED_MARKER} old -->\nstale")).unwrap();
        std::fs::write(root.join(CLAUDE_FILE), "my own notes").unwrap();
        let second = seed(paths, &root, cli).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(AGENTS_FILE)).unwrap(),
            agents_md(paths, cli)
        );
        assert_eq!(std::fs::read_to_string(root.join(CLAUDE_FILE)).unwrap(), "my own notes");
        assert!(second.contains(&SeedOutcome::KeptUserFile(root.join(CLAUDE_FILE))));
        let _ = std::fs::remove_dir_all(&root);
    }
}
