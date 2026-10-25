//! Modal patch scripts: an eseqlisp buffer whose last form is an
//! `(eseq.modal/patch ...)` turned into DGenLisp instrument source by the
//! `eseq.modal` module (`content/ui/modal.lisp`). Shared by `eseq modal emit`
//! and the app's draft evaluation, so both report the same errors.

use std::path::{Path, PathBuf};

use eseqlisp::vm::{Value, VM};

use crate::sequencer::CustomInstrumentRunMode;

/// Where the evaluated patch is parked while `eseq.modal/generate` runs; a
/// plain host-owned global, overwritten on every call.
const PATCH_SLOT: &str = "eseq-modal-patch-under-generation";

/// A generated instrument: the name the patch asked for, its dsp.lisp and
/// its ui.lisp panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModalInstrument {
    pub name: String,
    pub dsp: String,
    pub ui: String,
}

impl ModalInstrument {
    /// Write dsp.lisp, ui.lisp and instrument.json into `dir`, creating it.
    pub fn write_to(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let metadata = format!(
            "{{\"version\": 1, \"run_mode\": \"{}\"}}\n",
            CustomInstrumentRunMode::Instrument.as_str()
        );
        for (name, text) in [("dsp.lisp", &self.dsp), ("ui.lisp", &self.ui), ("instrument.json", &metadata)] {
            let file = dir.join(name);
            std::fs::write(&file, text).map_err(|error| format!("{}: {error}", file.display()))?;
        }
        Ok(())
    }
}

/// True when `source` imports `eseq.modal` (not a longer module name such as
/// `eseq.modal-x`). Cheap textual test for routing an evaluation.
pub fn is_modal_patch_script(source: &str) -> bool {
    source.match_indices("(import eseq.modal").any(|(at, needle)| {
        source[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_whitespace() || next == ')')
    })
}

/// Evaluate `script` (attributed to `path`) in `vm`, then check and generate
/// the patch its last form returns. Errors are user-facing lines: a script
/// that fails to evaluate, a last form that is not a patch, or every problem
/// `eseq.modal/check` finds.
pub fn generate_modal_instrument(
    host: &mut impl ModalScriptHost,
    path: PathBuf,
    script: &str,
) -> Result<ModalInstrument, Vec<String>> {
    let value = host.eval_at(path, script)?;
    if !is_patch(value.as_ref()) {
        return Err(vec![
            "the script's last form must be a patch: (m/patch :objects ... :listens ...)".to_string(),
        ]);
    }
    host.set_global(PATCH_SLOT, value.unwrap_or(Value::Nil));
    let generated = host.eval_at(
        PathBuf::from("<eseq.modal generate>"),
        &format!("(import eseq.modal)\n(eseq.modal/generate {PATCH_SLOT})"),
    )?;
    let Some(Value::Map(map)) = generated else {
        return Err(vec!["eseq.modal returned no instrument".to_string()]);
    };
    if let Some(errors) = map.get("errors") {
        return Err(string_list(&errors.borrow()));
    }
    let text = |key: &str| match map.get(key).map(|cell| cell.borrow().clone()) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    };
    match (text("dsp"), text("ui")) {
        (Some(dsp), Some(ui)) => Ok(ModalInstrument {
            name: text("name").unwrap_or_else(|| "modal".to_string()),
            dsp,
            ui,
        }),
        _ => Err(vec!["eseq.modal returned no dsp or ui source".to_string()]),
    }
}

/// Where a modal script runs: the CLI's bare VM or the app's runtime.
pub trait ModalScriptHost {
    /// Evaluate `source` as the file `path`; errors are user-facing lines.
    fn eval_at(&mut self, path: PathBuf, source: &str) -> Result<Option<Value>, Vec<String>>;
    fn set_global(&mut self, name: &str, value: Value);
}

impl ModalScriptHost for VM {
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

impl ModalScriptHost for eseqlisp::Runtime {
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

fn is_patch(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Map(map)) if map.contains_key("objects") && map.contains_key("listens"))
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
mod tests {
    use super::*;

    fn modal_vm() -> VM {
        let mut vm = VM::new(Vec::new());
        eseqlisp::vm::register_core_natives(&mut vm);
        eseqlisp::vm::register_math_natives(&mut vm);
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        vm.source_manager.set_module_load_roots(vec![content]);
        vm
    }

    const CELLO: &str = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-cello\"
  :objects (list (m/string 'str :made-of mat/nylon) (m/plate 'body))
  :connections (list (m/bow 'bow 'str 0.12)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
  :listens (list (m/listen 'str 0.85)))";

    #[test]
    fn generates_grouped_dsp_from_a_patch_script() {
        let mut vm = modal_vm();
        let instrument =
            generate_modal_instrument(&mut vm, PathBuf::from("cello.lisp"), CELLO).unwrap();
        assert_eq!(instrument.name, "probe-cello");
        assert!(instrument.dsp.contains("(param str_length @group str"), "{}", instrument.dsp);
        assert!(instrument.dsp.contains("(param bow_pressure @group bow"));
        assert!(instrument.dsp.contains("bow-contact"));
        assert!(instrument.ui.contains("(defsynth-ui"), "{}", instrument.ui);
        assert!(instrument.ui.contains("\"bow.bow_pressure\""), "{}", instrument.ui);
    }

    #[test]
    fn generates_in_the_app_runtime_after_user_definitions() {
        // The app evaluates scripts in its UI runtime, where a script's own
        // defs land beside everything else; a script may compute its patch.
        let mut runtime = eseqlisp::Runtime::new();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        runtime.set_module_load_path(vec![content]);
        let script = "(import eseq.modal :as m)
(def ratios (list 1.5 2))
(def drone (k) (m/string (symbol \"sym\" k) :tune (nth ratios k) :mute 0))
(m/patch :name \"drones\"
  :objects (append (list (m/plate 'body)) (map drone (range (len ratios))))
  :connections (map (lambda (k) (m/spring (symbol \"b\" k) (symbol \"sym\" k) 0.96 'body (list 0.5 0.5)))
                    (range (len ratios)))
  :listens (list (m/listen 'body (list 0.3 0.6))))";
        let instrument =
            generate_modal_instrument(&mut runtime, PathBuf::from("drones.lisp"), script).unwrap();
        assert_eq!(instrument.name, "drones");
        assert!(instrument.dsp.contains("(param sym1_tune @group sym1 @default 2"), "{}", instrument.dsp);
        assert!(instrument.dsp.contains("(param b0_stiff @group b0"));
    }

    #[test]
    fn reports_patch_problems_by_name() {
        let mut vm = modal_vm();
        let script = CELLO.replace("'bridge 'str 0.99 'body", "'bridge 'str 0.99 'bdy");
        let errors =
            generate_modal_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
        assert_eq!(errors, vec!["spring bridge: no object bdy".to_string()]);
    }

    #[test]
    fn a_list_of_materials_becomes_a_menu_and_one_becomes_numbers() {
        let mut vm = modal_vm();
        let script = CELLO.replace(
            "(m/plate 'body)",
            "(m/plate 'body :made-of (list mat/spruce mat/maple (mat/material :name \"cedar\" :density 380 :young 9e9)))",
        );
        let instrument =
            generate_modal_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap();
        let dsp = &instrument.dsp;
        // One material: its numbers, and no menu.
        assert!(dsp.contains("(def str_density 1140)"), "{dsp}");
        assert!(dsp.contains("(def str_young 3000000000)"), "{dsp}");
        assert!(!dsp.contains("str_material"), "{dsp}");
        // Several: a menu param named after them, picked by its index.
        assert!(dsp.contains("(param body_material @group body @options [\"spruce\" \"maple\" \"cedar\"]"), "{dsp}");
        assert!(dsp.contains("(selector (+ 1 (clip (round body_material) 0 2)) 450 650 380)"), "{dsp}");
        assert!(instrument.ui.contains("\"body.body_material\""), "{}", instrument.ui);
    }

    #[test]
    fn rejects_a_keyword_material() {
        let mut vm = modal_vm();
        let script = CELLO.replace("mat/nylon", ":nylon");
        let errors =
            generate_modal_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
        assert_eq!(
            errors,
            vec!["string str: :made-of takes a material such as mat/steel, or a list of them; got :nylon".to_string()]
        );
    }

    #[test]
    fn checks_every_material_field() {
        let mut vm = modal_vm();
        for (made_of, expected) in [
            ("nil", "string str: :made-of takes a material such as mat/steel, or a list of them; got nil"),
            ("(mat/material :name \"gut\" :density nil)", "string str: material gut needs a number :density"),
            ("(list mat/nylon (merge mat/steel :name \"nylon\"))", "string str: two materials are called nylon"),
        ] {
            let script = CELLO.replace("mat/nylon)", &format!("{made_of})"));
            let errors = generate_modal_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
            assert_eq!(errors, vec![expected.to_string()], "{made_of}");
        }
    }

    #[test]
    fn routes_only_scripts_that_import_eseq_modal() {
        assert!(is_modal_patch_script("(import eseq.modal :as m)\n(m/patch)"));
        assert!(is_modal_patch_script("(import eseq.modal)"));
        assert!(!is_modal_patch_script("(import eseq.modal-tools :as t)"));
        assert!(!is_modal_patch_script("(def x 1)"));
    }

    #[test]
    fn rejects_a_script_that_does_not_end_in_a_patch() {
        let mut vm = modal_vm();
        let errors = generate_modal_instrument(
            &mut vm,
            PathBuf::from("x.lisp"),
            "(import eseq.modal :as m)\n42",
        )
        .unwrap_err();
        assert!(errors[0].contains("last form must be a patch"), "{errors:?}");
    }
}
