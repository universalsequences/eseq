//! `metal_seq noui [FILE] [--cwd DIR]` (eseq-750i): the full host — audio
//! engine, sequencer natives, user init.lisp, packages — under a bare Lisp
//! root (`ui/noui.lisp`) instead of the DAW layout, with FILE open full-window
//! and the process working directory set to the shell's, so `find-file` and
//! `dired` start where the session was launched and `(import name)` finds
//! modules in that directory.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoUiArgs {
    /// Directory the session was launched from; becomes the process cwd once
    /// the editor is up.
    pub(crate) cwd: PathBuf,
    /// File to visit, absolute. Created on first save if it does not exist.
    pub(crate) file: Option<PathBuf>,
}

impl NoUiArgs {
    /// `None` unless the first argument is `noui`. Must run before
    /// `enter_sequencer_dir`, which replaces the process cwd in checkouts.
    pub(crate) fn parse_env() -> Result<Option<Self>, String> {
        let cwd = std::env::current_dir()
            .map_err(|error| format!("failed to read the current directory: {error}"))?;
        Self::parse(std::env::args().skip(1), &cwd)
    }

    fn parse(mut args: impl Iterator<Item = String>, cwd: &Path) -> Result<Option<Self>, String> {
        if args.next().as_deref() != Some("noui") {
            return Ok(None);
        }
        let mut cwd = cwd.to_path_buf();
        let mut file = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--cwd" => {
                    let dir = args.next().ok_or("--cwd requires a directory")?;
                    cwd = absolute(&cwd, PathBuf::from(dir));
                }
                "-h" | "--help" => return Err(Self::usage().to_string()),
                flag if flag.starts_with("--") => {
                    return Err(format!("unknown noui argument {flag}\n{}", Self::usage()));
                }
                path if file.is_none() => file = Some(PathBuf::from(path)),
                extra => return Err(format!("unexpected argument {extra}\n{}", Self::usage())),
            }
        }
        // Resolve FILE against the final cwd so `--cwd` order does not matter.
        let file = file.map(|file| absolute(&cwd, file));
        Ok(Some(Self { cwd, file }))
    }

    fn usage() -> &'static str {
        "usage: metal_seq noui [FILE] [--cwd DIR]"
    }
}

fn absolute(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Point the finished editor at the launch directory and the requested file.
pub(crate) fn enter_session(
    editor: &mut eseqlisp::Editor,
    args: &NoUiArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    // Every load root is absolute by now, so handing the process cwd back to
    // the shell's directory only changes where find-file / dired start.
    std::env::set_current_dir(&args.cwd)
        .map_err(|error| format!("failed to enter {}: {error}", args.cwd.display()))?;
    // The launch directory is the session's project root: `(import utils)`
    // finds ./utils.lisp and `(import lib.drums)` ./lib/drums.lisp, ahead of
    // local, installed and factory modules.
    let (mut roots, _package_errors) = sequencer::app_paths::app_paths().module_load_roots();
    roots.insert(0, eseqlisp::ModuleLoadRoot { path: args.cwd.clone(), module_prefix: None });
    editor.runtime_mut().set_scoped_module_load_path(roots);
    if let Some(file) = &args.file {
        editor
            .open_or_create_file_buffer(file)
            .map_err(|error| format!("failed to open {}: {error:?}", file.display()))?;
    }
    editor
        .runtime_mut()
        .eval_str("(delete-other-windows)")
        .map_err(|error| format!("failed to collapse to one window: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<NoUiArgs>, String> {
        NoUiArgs::parse(args.iter().map(|a| a.to_string()), Path::new("/shell"))
    }

    #[test]
    fn other_commands_are_not_noui() {
        assert_eq!(parse(&[]), Ok(None));
        assert_eq!(parse(&["capture", "--script", "x"]), Ok(None));
    }

    #[test]
    fn file_resolves_against_launch_cwd() {
        assert_eq!(
            parse(&["noui", "exp/a.lisp"]),
            Ok(Some(NoUiArgs {
                cwd: PathBuf::from("/shell"),
                file: Some(PathBuf::from("/shell/exp/a.lisp")),
            }))
        );
        assert_eq!(
            parse(&["noui"]),
            Ok(Some(NoUiArgs { cwd: PathBuf::from("/shell"), file: None }))
        );
    }

    #[test]
    fn explicit_cwd_wins_regardless_of_order() {
        let expected = Ok(Some(NoUiArgs {
            cwd: PathBuf::from("/lisp/exp"),
            file: Some(PathBuf::from("/lisp/exp/a.lisp")),
        }));
        assert_eq!(parse(&["noui", "a.lisp", "--cwd", "/lisp/exp"]), expected);
        assert_eq!(parse(&["noui", "--cwd", "/lisp/exp", "a.lisp"]), expected);
    }

    #[test]
    fn launch_directory_is_a_module_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        std::fs::write(
            dir.path().join("lib/beats.lisp"),
            "(module lib.beats)\n(export double)\n(def double (n) (* 2 n))\n",
        )
        .unwrap();
        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        let args = NoUiArgs { cwd: dir.path().to_path_buf(), file: None };
        enter_session(&mut editor, &args).unwrap();

        let value = editor
            .runtime_mut()
            .eval_str("(do (import lib.beats) (lib.beats/double 21))")
            .unwrap();
        assert_eq!(value, Some(eseqlisp::vm::Value::Number(42.0)));
    }

    #[test]
    fn rejects_unknown_flags_and_extra_files() {
        assert!(parse(&["noui", "--bogus"]).is_err());
        assert!(parse(&["noui", "a.lisp", "b.lisp"]).is_err());
        assert!(parse(&["noui", "--cwd"]).is_err());
    }
}
