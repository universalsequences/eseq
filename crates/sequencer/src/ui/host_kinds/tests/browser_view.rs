//! The factory browser, sample import (with their shared preview strip),
//! resample and sound palette views, ported to the kinds (kind-bindings
//! spec §13 stage 8, eseq-0l17.17).

use super::views::{assert_ported, distro, instance_bindings};
use super::*;

impl Harness {
    /// Evaluate `code` with the kinds these views read referred.
    fn eval_browser(&mut self, code: &str) -> Value {
        let source = format!("(import eseq.kinds :refer (project browser))\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }
}

/// The ported views' sources.
const PORTED: [(&str, &str); 5] = [
    (
        "ui/browser.lisp",
        include_str!("../../../../../../content/ui/browser.lisp"),
    ),
    (
        "ui/sample-import.lisp",
        include_str!("../../../../../../content/ui/sample-import.lisp"),
    ),
    (
        "ui/preview-strip.lisp",
        include_str!("../../../../../../content/ui/preview-strip.lisp"),
    ),
    (
        "ui/resample.lisp",
        include_str!("../../../../../../content/ui/resample.lisp"),
    ),
    (
        "ui/sound-palette.lisp",
        include_str!("../../../../../../content/ui/sound-palette.lisp"),
    ),
];

#[test]
fn ported_browser_views_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
    for (file, source) in PORTED {
        assert!(!source.contains("RESAMPLE."), "{file} reads RESAMPLE");
    }
}

/// The sample preview strip binds the preview player's position (a live
/// field) and the browser lists the package instances from the project.
#[test]
fn the_browser_binds_the_preview_through_kinds_and_lists_package_instances() {
    let mut h = distro();
    h.eval_browser(
        "(let ((v eseq.browser/browser-view) (p eseq.browser/sample-preview))
           (set! v.tab \"samples\")
           (set! p.path \"samples/kick.wav\")
           (set! p.buffer (dict :duration 1.5)))",
    );
    h.sync();
    h.show_all();
    let (tree, revision) = h.buffer_tree("*samples*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    let browser = h.singleton(BROWSER);
    assert!(
        bound.contains(&(browser, "preview-position".to_string())),
        "the strip's playhead binds browser.preview-position: {bound:?}"
    );
    // An idle sync re-renders nothing.
    h.sync();
    h.show_all();
    assert_eq!(h.buffer_tree("*samples*").1, revision);

    // The package instances, as the Packages tree takes them: pushed only
    // while observed, so the Samples tab leaves them unpushed.
    assert_eq!(
        h.eval_browser("(let ((p project)) p.instances)"),
        Value::List(vec![])
    );
    h.app
        .instances
        .list
        .push(sequencer::project::ProjectInstance {
            id: 7,
            kind: "alez/neural:neural".to_string(),
            owner: Default::default(),
            label: "neural 1".to_string(),
        });
    h.sync();
    assert_eq!(
        h.eval_browser("(len project.instances)"),
        Value::Number(0.0),
        "unobserved, project.instances is not pushed"
    );
    // The Packages tree observes them: the next sync pushes.
    h.eval_browser("(eseq.browser/select-tab \"packages\")");
    h.show_all();
    h.sync();
    assert_eq!(
        h.eval_browser("(len project.instances)"),
        Value::Number(1.0)
    );
    assert_eq!(
        h.eval_browser("(get (first project.instances) :label)"),
        Value::String("neural 1".to_string())
    );
    assert_eq!(
        h.eval_browser("(get (first project.instances) :id)"),
        Value::Number(7.0)
    );
}

/// Resample reports a failed command through the modal's own state, not a
/// host namespace; a fresh print clears it.
#[test]
fn resample_errors_and_prints_land_in_the_modal_state() {
    let mut h = distro();
    let view = |h: &mut Harness, field: &str| {
        h.eval_browser(&format!(
            "(let ((v eseq.resample/resample-view)) v.{field})"
        ))
    };
    h.eval_browser("(eseq.resample/show-error \"Nothing has played yet\")");
    assert_eq!(
        view(&mut h, "error"),
        Value::String("Nothing has played yet".to_string())
    );
    h.eval_browser("(eseq.resample/open 0.5 2 \"Print\" (list \"Resampled\") false 4)");
    assert_eq!(view(&mut h, "error"), Value::String(String::new()));
    assert_eq!(view(&mut h, "duration"), Value::Number(4.0));
    assert_eq!(view(&mut h, "open"), Value::Bool(true));
    h.eval_browser("(eseq.resample/close)");
    assert_eq!(view(&mut h, "open"), Value::Bool(false));
    assert_eq!(view(&mut h, "buffer"), Value::Bool(false));
}
