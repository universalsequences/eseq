//! Program generators (docs/program-generators-spec.md): an eseqlisp script
//! whose last value is a program (a map with a callable `:generator`) turned
//! into an instrument by `eseq.gen/generate` (`content/ui/gen.lisp`). Nothing
//! here knows which generator ran; `eseq.modal` is one. Shared by
//! `eseq generate` and the app's C-c C-c drafts, so both report the same
//! errors.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use eseqlisp::vm::{Value, VM};

use crate::sequencer::CustomInstrumentRunMode;

/// Where the evaluated program is parked while `eseq.gen/generate` runs; a
/// plain host-owned global, overwritten on every call.
const PROGRAM_SLOT: &str = "eseq-program-under-generation";

/// A generated instrument, as a generator's result map gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedInstrument {
    /// Its folder and engine name.
    pub name: String,
    pub dsp: String,
    /// The ui.lisp panel; none for the stock panel.
    pub ui: Option<String>,
    /// The param the host binds its voice mode to (instrument.json
    /// `voice_controls`).
    pub voice_mode: Option<String>,
    /// Files ui.lisp reads by absolute path, copied beside it on write.
    pub assets: Vec<PathBuf>,
}

impl GeneratedInstrument {
    /// Write dsp.lisp, ui.lisp (when there is one) and instrument.json into
    /// `dir`, creating it. Each absolute asset is copied beside them and
    /// ui.lisp points at the copy, so the folder depends on no cache or
    /// draft folder.
    pub fn write_to(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let mut metadata = serde_json::json!({
            "version": 1,
            "run_mode": CustomInstrumentRunMode::Instrument.as_str(),
        });
        if let Some(param) = &self.voice_mode {
            metadata["voice_controls"] = serde_json::json!({ "mode": param });
        }
        let mut ui = self.ui.clone();
        // two assets of one file name from different folders get a numbered
        // copy each, not one copy both point at
        let mut taken: std::collections::HashMap<PathBuf, &Path> = std::collections::HashMap::new();
        for from in self.assets.iter().filter(|path| path.is_absolute()) {
            let Some(name) = from.file_name() else { continue };
            let mut to = dir.join(name);
            for k in 2.. {
                match taken.get(&to) {
                    Some(source) if *source != from.as_path() => {
                        to = dir.join(format!("{k}-{}", name.to_string_lossy()));
                    }
                    _ => break,
                }
            }
            if taken.insert(to.clone(), from).is_some() {
                continue;
            }
            if *from != to {
                std::fs::copy(from, &to).map_err(|error| format!("{}: {error}", from.display()))?;
            }
            if let Some(ui) = ui.as_mut() {
                *ui = ui.replace(&from.display().to_string(), &to.display().to_string());
            }
        }
        let metadata = format!("{metadata}\n");
        let mut files = vec![("dsp.lisp", &self.dsp), ("instrument.json", &metadata)];
        if let Some(ui) = &ui {
            files.push(("ui.lisp", ui));
        }
        for (name, text) in files {
            let file = dir.join(name);
            std::fs::write(&file, text).map_err(|error| format!("{}: {error}", file.display()))?;
        }
        Ok(())
    }
}

thread_local! {
    /// The folder of the script being generated, which natives reading files
    /// relative to the script (`modal-read-data`) resolve against.
    static SCRIPT_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The folder of the script being generated, while it is.
pub fn current_script_dir() -> Option<PathBuf> {
    SCRIPT_DIR.with(|dir| dir.borrow().clone())
}

/// True when `value` is a program: a map whose `:generator` is callable.
pub fn is_program(value: &Value) -> bool {
    let Value::Map(map) = value else { return false };
    map.get("generator").is_some_and(|generator| eseqlisp::vm::is_callable(&generator.borrow()))
}

/// Run `f` with [`current_script_dir`] the folder of the script `path`.
fn in_script_dir<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let previous = SCRIPT_DIR.with(|cell| cell.replace(path.parent().map(Path::to_path_buf)));
    let value = f();
    SCRIPT_DIR.with(|cell| *cell.borrow_mut() = previous);
    value
}

/// Evaluate `script` (attributed to `path`) in `host`; its last value.
/// While it runs, [`current_script_dir`] is the script's folder.
pub fn eval_script(
    host: &mut impl ScriptHost,
    path: PathBuf,
    script: &str,
) -> Result<Option<Value>, Vec<String>> {
    let dir_of = path.clone();
    in_script_dir(&dir_of, || host.eval_at(path, script))
}

/// Evaluate `script` and generate the instrument its last value, a program,
/// describes. Errors are user-facing lines: a script that fails to evaluate,
/// a last value that is not a program, or what the generator reports.
pub fn generate_instrument(
    host: &mut impl ScriptHost,
    path: PathBuf,
    script: &str,
) -> Result<GeneratedInstrument, Vec<String>> {
    let program = eval_script(host, path.clone(), script)?.unwrap_or(Value::Nil);
    generate_from_program(host, path, program)
}

/// Generate the instrument `program` (a script's last value, evaluated from
/// `path`) describes; see [`generate_instrument`]. `eseq.gen/generate`
/// reports a value that is not a program.
pub fn generate_from_program(
    host: &mut impl ScriptHost,
    path: PathBuf,
    program: Value,
) -> Result<GeneratedInstrument, Vec<String>> {
    host.set_global(PROGRAM_SLOT, program);
    // The generator may read files relative to the script, as its
    // evaluation did.
    let generated = in_script_dir(&path, || {
        host.eval_at(
            PathBuf::from("<eseq.gen generate>"),
            &format!("(import eseq.gen)\n(eseq.gen/generate {PROGRAM_SLOT})"),
        )
    });
    // The runtime is long-lived: do not keep the program (and its data) alive.
    host.set_global(PROGRAM_SLOT, Value::Nil);
    let Some(Value::Map(map)) = generated? else {
        return Err(vec!["the generator returned no instrument".to_string()]);
    };
    if let Some(errors) = map.get("errors") {
        return Err(string_list(&errors.borrow()));
    }
    let text = |key: &str| match map.get(key).map(|cell| cell.borrow().clone()) {
        Some(Value::String(text)) => Some(text),
        Some(Value::Symbol(text) | Value::Keyword(text)) => Some(text.trim_start_matches(':').to_string()),
        _ => None,
    };
    let assets = match map.get("assets").map(|cell| cell.borrow().clone()) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::String(path) => Some(PathBuf::from(path)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    // eseq.gen/generate checked the result's shape; the name is also the
    // instrument's folder, so it must be one plain path component.
    let name = text("name").unwrap_or_default();
    let mut components = Path::new(&name).components();
    if !matches!((components.next(), components.next()), (Some(std::path::Component::Normal(_)), None)) {
        return Err(vec![format!("the generator's :name {name:?} must be a file name, not a path")]);
    }
    Ok(GeneratedInstrument {
        name,
        dsp: text("dsp").unwrap_or_default(),
        ui: text("ui"),
        voice_mode: text("voice-mode"),
        assets,
    })
}

/// Where a script runs: the CLI's bare VM or the app's runtime.
pub trait ScriptHost {
    /// Evaluate `source` as the file `path`; errors are user-facing lines.
    fn eval_at(&mut self, path: PathBuf, source: &str) -> Result<Option<Value>, Vec<String>>;
    fn set_global(&mut self, name: &str, value: Value);
}

impl ScriptHost for VM {
    fn eval_at(&mut self, path: PathBuf, source: &str) -> Result<Option<Value>, Vec<String>> {
        let seen = self.source_manager.diagnostics_len();
        self.eval_module_source(path.clone(), source, 0).map_err(|error| {
            let mut lines = vec![format!("{}: {error:?}", path.display())];
            lines.extend(self.take_source_load_errors());
            lines.extend(self.source_manager.diagnostics_since(seen));
            lines
        })
    }

    fn set_global(&mut self, name: &str, value: Value) {
        self.set_global_value(name, value);
    }
}

impl ScriptHost for eseqlisp::Runtime {
    fn eval_at(&mut self, path: PathBuf, source: &str) -> Result<Option<Value>, Vec<String>> {
        // eval_source_at_path moves its load errors into the diagnostics.
        let seen = self.source_diagnostics_len();
        self.eval_source_at_path(path.clone(), source).map_err(|error| {
            let mut lines = vec![format!("{}: {error:?}", path.display())];
            lines.extend(self.source_diagnostics_since(seen));
            lines
        })
    }

    fn set_global(&mut self, name: &str, value: Value) {
        self.set_global_value(name, value);
    }
}

fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::String(text) => text.clone(),
                other => format!("{other:?}"),
            })
            .collect(),
        other => vec![format!("{other:?}")],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A bare VM whose imports resolve to the checkout's content.
    pub(crate) fn gen_vm() -> VM {
        let mut vm = VM::new(Vec::new());
        eseqlisp::vm::register_core_natives(&mut vm);
        eseqlisp::vm::register_math_natives(&mut vm);
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        vm.source_manager.set_module_load_roots(vec![content]);
        vm
    }

    /// A generator local to the script: no module, no registration.
    const SINE: &str = "(import eseq.gen :as gen)
(def sine-synth (p)
  (dict :name (get p :name)
        :dsp (str \"(def gate (in 1 @name gate))\\n(out (* gate (sin (* 6.283 (phasor \" (get p :hz) \")))) 1 @name audio)\\n\")))
(gen/program sine-synth :name \"sine\" :hz 220)";

    #[test]
    fn generates_from_a_generator_the_script_defines() {
        let instrument = generate_instrument(&mut gen_vm(), PathBuf::from("sine.lisp"), SINE).unwrap();
        assert_eq!(instrument.name, "sine");
        assert!(instrument.dsp.contains("(phasor 220)"), "{}", instrument.dsp);
        assert_eq!((instrument.ui, instrument.voice_mode, instrument.assets), (None, None, Vec::new()));
    }

    #[test]
    fn rejects_a_value_that_is_not_a_program() {
        for last in ["42", "(dict :name \"x\")", "(dict :generator 3)"] {
            let errors =
                generate_instrument(&mut gen_vm(), PathBuf::from("x.lisp"), &format!("(import eseq.gen)\n{last}"))
                    .unwrap_err();
            assert!(errors[0].contains("must be a program"), "{last}: {errors:?}");
        }
    }

    #[test]
    fn checks_the_generators_result_by_name() {
        let cases = [
            ("(lambda (p) 7)", "the generator returned 7, not a result map"),
            ("(lambda (p) (dict :dsp \"\"))", "the generator's result has no :name"),
            ("(lambda (p) (dict :name \"x\"))", "the generator's result has no :dsp source"),
            ("(lambda (p) (dict :name \"\" :dsp \"\"))", "the generator's result has no :name"),
            ("(lambda (p) (dict :name \"x\" :dsp \"\" :ui 3))", "the generator's :ui must be ui.lisp source, a string"),
            ("(lambda (p) (dict :name \"x\" :dsp \"\" :assets (list 1)))", "the generator's :assets must be a list of file paths"),
            ("(lambda (p) (dict :name \"x\" :dsp \"\" :kind :effect))", "the generator made a :effect; only :instrument is supported"),
            ("(lambda (p) (dict :errors (list \"no strings\")))", "no strings"),
        ];
        for (generator, error) in cases {
            let script = format!("(import eseq.gen :as gen)\n(gen/program {generator})");
            let errors = generate_instrument(&mut gen_vm(), PathBuf::from("x.lisp"), &script).unwrap_err();
            assert_eq!(errors, vec![error.to_string()], "{generator}");
        }
    }

    /// tools/gen/examples/additive.lisp: a generator built from eseq.gen
    /// alone (no modal code) compiles, makes sound and has a valid panel.
    #[test]
    fn the_additive_example_compiles_and_sounds() {
        use crate::agent::verify::{verify_sources, ArtifactKind, VerifyMode};
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen/examples/additive.lisp");
        let script = std::fs::read_to_string(&path).unwrap();
        let instrument = generate_instrument(&mut gen_vm(), path, &script).unwrap();
        assert_eq!(instrument.name, "gen-additive");
        // A note-following knob, modulatable; setup-fed tensor math is not.
        assert!(instrument.dsp.contains("(param tone_bright_c4 @group tone @default 0.788 @min 0 @max 1 @mod true"));
        assert!(instrument.dsp.contains("(param tone_tilt @group tone @default -6 @min -24 @max 0)"));
        let ui = instrument.ui.as_deref().unwrap();
        let report = verify_sources(ArtifactKind::Instrument, VerifyMode::Library, &instrument.dsp, Some(ui), 44_100, None)
            .unwrap_or_else(|failure| panic!("{failure}\n{}", instrument.dsp));
        assert!(!report.audition.silent);
    }

    #[test]
    fn writes_metadata_and_relocates_assets() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("shape.modes.json");
        std::fs::write(&asset, "{}").unwrap();
        let instrument = GeneratedInstrument {
            name: "x".to_string(),
            dsp: "(out 0 1 @name audio)".to_string(),
            ui: Some(format!("(view :src \"{}\")", asset.display())),
            voice_mode: Some("out_voice_mode".to_string()),
            assets: vec![asset.clone()],
        };
        let out = dir.path().join("out");
        instrument.write_to(&out).unwrap();
        let ui = std::fs::read_to_string(out.join("ui.lisp")).unwrap();
        assert_eq!(ui, format!("(view :src \"{}\")", out.join("shape.modes.json").display()));
        assert!(out.join("shape.modes.json").exists());
        let metadata = std::fs::read_to_string(out.join("instrument.json")).unwrap();
        assert!(metadata.contains(r#""voice_controls":{"mode":"out_voice_mode"}"#), "{metadata}");

        let bare = GeneratedInstrument { ui: None, voice_mode: None, assets: Vec::new(), ..instrument };
        let out = dir.path().join("bare");
        bare.write_to(&out).unwrap();
        assert!(!out.join("ui.lisp").exists());
        assert!(!std::fs::read_to_string(out.join("instrument.json")).unwrap().contains("voice_controls"));
    }

    /// Two assets of one file name from different folders each keep their
    /// own copy, and ui.lisp points at each.
    #[test]
    fn assets_sharing_a_file_name_both_survive() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a/ride.modes.json"), dir.path().join("b/ride.modes.json"));
        for (path, text) in [(&a, "a"), (&b, "b")] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let instrument = GeneratedInstrument {
            name: "x".to_string(),
            dsp: String::new(),
            ui: Some(format!("{}|{}", a.display(), b.display())),
            voice_mode: None,
            assets: vec![a.clone(), b.clone(), a.clone()],
        };
        let out = dir.path().join("out");
        instrument.write_to(&out).unwrap();
        let (first, second) = (out.join("ride.modes.json"), out.join("2-ride.modes.json"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "b");
        let ui = std::fs::read_to_string(out.join("ui.lisp")).unwrap();
        assert_eq!(ui, format!("{}|{}", first.display(), second.display()));
    }

    /// The name is the instrument's folder: a path is refused.
    #[test]
    fn refuses_a_name_that_is_a_path() {
        for name in ["../up", "/tmp/x", "a/b", ".."] {
            let script = format!("(import eseq.gen :as gen)\n(gen/program (lambda (p) (dict :name {name:?} :dsp \"\")))");
            let errors = generate_instrument(&mut gen_vm(), PathBuf::from("x.lisp"), &script).unwrap_err();
            assert!(errors[0].contains("must be a file name"), "{name}: {errors:?}");
        }
    }
}
