//! The song export, Agent Mode and Promote to factory views ported to the
//! kinds (kind-bindings spec §13 stage 8, eseq-0l17.76): their host state is
//! the `song-export`, `agent` and `factory-promote` singletons, their own
//! state a `:state` singleton each (`export-draft`, `agent-chat`,
//! `promote-form`).

use super::graph_demos_view::show_on_screen;
use super::views::{assert_ported, distro, tree_has_string_prop, widget_keyed};
use super::*;
use crate::presented::{present_promote, presented, PromoteView};
use sequencer::bounce::job::WorkerStatus;

/// The ported views' sources.
const PORTED: [(&str, &str); 3] = [
    (
        "ui/export-song.lisp",
        include_str!("../../../../../../content/ui/export-song.lisp"),
    ),
    (
        "ui/agent.lisp",
        include_str!("../../../../../../content/ui/agent.lisp"),
    ),
    (
        "ui/factory-promote.lisp",
        include_str!("../../../../../../content/ui/factory-promote.lisp"),
    ),
];

#[test]
fn ported_modal_views_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

/// The export modal shows each job transition as the host kinds push it,
/// with no reopening.
#[test]
fn the_export_modal_follows_the_job_through_the_kinds() {
    let mut h = distro();
    show_on_screen(&mut h, "*sequencer*");
    h.eval("(eseq.export-song/open)");
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*sequencer*");
    assert!(widget_keyed(&tree, "export-submit").is_some());
    assert!(widget_keyed(&tree, "export-cancel").is_none());
    crate::host_commands::export::publish_job_status(
        &mut h.editor,
        &WorkerStatus::Rendering { percent: 37 },
        true,
    );
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*sequencer*");
    assert!(widget_keyed(&tree, "export-cancel").is_some());
    assert!(widget_keyed(&tree, "export-submit").is_none());
    assert!(tree_has_string_prop(&tree, "text", "Exporting audio — 37%"));
    crate::host_commands::export::publish_job_status(
        &mut h.editor,
        &WorkerStatus::Completed {
            frames: 1,
            tail_warning: false,
        },
        false,
    );
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*sequencer*");
    assert!(widget_keyed(&tree, "export-reveal").is_some());
    assert!(tree_has_string_prop(&tree, "text", "Export complete."));
}

/// Agent Mode re-renders when an agent session changes
/// (`agent.generation`), and not on an idle tick.
#[test]
fn agent_mode_rerenders_on_the_agent_generation() {
    let mut h = distro();
    show_on_screen(&mut h, "*agent*");
    let (_, before) = h.buffer_tree("*agent*");
    let generation = presented(|p| *p.agent.get()) + 1;
    present_agent(h.editor.runtime_mut(), generation);
    h.pkg_render();
    let (_, after) = h.buffer_tree("*agent*");
    assert!(after > before, "{before} -> {after}");
    h.pkg_render();
    assert_eq!(h.buffer_tree("*agent*").1, after);
}

/// The promote modal shows the presented promotion; a name the host found
/// taken turns Promote into Replace, and the commit says so.
#[test]
fn the_promote_modal_reads_the_presented_promotion() {
    let mut h = distro();
    show_on_screen(&mut h, "*sequencer*");
    present_promote(|view| {
        *view = PromoteView {
            target: "kit".to_string(),
            destination: "content/kits/".to_string(),
            skipped: vec!["pad 'Kick': skipped".to_string()],
            taken: "chicken kit".to_string(),
            ..PromoteView::default()
        }
    });
    h.sync();
    h.eval("(eseq.factory-promote/open \"Chicken Kit\")");
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*sequencer*");
    assert!(tree_has_string_prop(&tree, "text", "Promote kit"));
    assert!(tree_has_string_prop(&tree, "text", "content/kits/"));
    assert!(widget_keyed(&tree, "skip-0").is_some());
    assert!(tree_has_string_prop(&tree, "text", "Replace factory copy"));
    h.eval("(eseq.factory-promote/commit)");
    let payload = h.last_custom("factory-promote-commit");
    let Value::Map(payload) = payload else {
        panic!("{payload:?}");
    };
    assert_eq!(*payload["name"].borrow(), s("Chicken Kit"));
    assert_eq!(*payload["overwrite"].borrow(), Value::Bool(true));
    // Blocked, the modal says why and commits nothing.
    present_promote(|view| {
        view.blocking = "not factory".to_string()
    });
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*sequencer*");
    assert!(widget_keyed(&tree, "blocking").is_some());
    h.custom_commands();
    h.eval("(eseq.factory-promote/commit)");
    assert!(h.custom_commands().is_empty());
}
