//! Schema checks: `eseq.kinds` against [`PUBLISHED`], hot reloads and module resolution.

use super::*;

#[test]
fn host_kinds_schema_matches_eseq_kinds() {
    let mut h = Harness::new();
    assert_eq!(check_schema(h.rt()), Ok(()));
    for name in host_kind_names() {
        let error = h
            .editor
            .runtime_mut()
            .eval_str(&format!("(def-kind {name} :key () :state ((open false)))"))
            .expect_err("reserved");
        assert!(
            format!("{error:?}").contains("reserved"),
            "{name}: {error:?}"
        );
    }
    // A drifted declaration is caught, naming both directions.
    h.eval(
        "(module eseq.kinds)
         (def-kind scene :key (index)
           :host ((index :int) (number :int) (name :string) (active :bool)
                  (queued :bool) (bank bank) (color :rgb)))
         (def-kind bank :key (index)
           :host ((index :int) (label :string) (scenes (list-of scene))))",
    );
    let errors = check_schema(h.rt()).expect_err("drift").join("\n");
    assert!(
        errors.contains("'color' is declared but the host never publishes it"),
        "{errors}"
    );
    assert!(
        errors.contains("publishes 'playing' (:bool), which is not a :host field"),
        "{errors}"
    );
}

#[test]
fn the_full_daw_root_loads_eseq_kinds_and_publishes_it() {
    let mut h = Harness::with_root(UiRoot::Distro);
    assert_eq!(check_schema(h.rt()), Ok(()));
    assert!(h.sync());
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    // The root imports the module without referring its names.
    let bare = h.editor.runtime_mut().eval_str("(tracks)");
    assert!(bare.is_err(), "kind names are not bare globals: {bare:?}");
}

#[test]
fn a_hot_reloaded_schema_drift_is_skipped_without_panicking() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def b0 (first (banks)))");
    // Hot reload a drifted eseq.kinds: bank loses `playing`, scene gains a
    // field the host never publishes and retypes `name`.
    h.eval(
        "(module eseq.kinds)
         (def-kind scene :key (index)
           :host ((index :int) (number :int) (name :int) (active :bool)
                  (queued :bool) (bank bank) (color :rgb)))
         (def-kind bank :key (index)
           :host ((index :int) (label :string) (scenes (list-of scene))))",
    );
    assert!(check_schema(h.rt()).is_err());
    // Pushes of the mismatched fields are skipped; the rest still flow.
    h.shared.state.pattern.track_params[0].set_volume(0.4);
    h.eval("(def t0 (track 0)) (def vol #'t0.volume)");
    h.sync();
    h.sync();
    assert!((h.slot("vol") - 0.4).abs() < 1e-6);
    let skip = h.frame.host_kinds.shared.borrow().skip.clone();
    assert!(
        skip.contains(&f::BANK_PLAYING) && skip.contains(&f::SCENE_NAME),
        "{skip:?}"
    );
    assert!(!skip.contains(&f::SCENE_INDEX));
    assert_eq!(
        h.eval("(let ((s (first (scenes)))) s.index)"),
        Value::Number(0.0)
    );
}

#[test]
fn startup_scripts_beside_the_core_modules_do_not_resolve_as_modules() {
    let mut h = Harness::new();
    for name in ["eseq.init", "eseq.sdf-stdlib"] {
        let result = h.editor.runtime_mut().eval_str(&format!("(import {name})"));
        let resolved = match &result {
            Err(_) => false,
            Ok(Some(Value::String(message))) => !message.contains("no module file found"),
            Ok(Some(_)) => true,
            Ok(None) => true,
        };
        assert!(!resolved, "{name} resolved: {result:?}");
    }
    // The core module itself does.
    h.eval("(import eseq.kinds)");
}
