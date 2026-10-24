//! The Projects tab's peek and import (docs/rig-spec.md §8, eseq-ka5t.14):
//! a project file opened read-only as a tree in the built-in browser, and a
//! pick imported from it through the host command the view queues.

use super::views::{distro, widget_keyed};
use super::*;

const REFER_PEEK: &str = "(import eseq.project-browser :refer (peek-project! toggle-pick! import-picks!))";
const PLATE: &str = "../../content/impulses/lexicon-300-rich-plate.wav";

impl Harness {
    /// Apply what Lisp queued, then sync and render, as one loop turn.
    fn peek_settle(&mut self) {
        self.drain();
        self.share_buses_and_groups();
        self.sync();
        self.show_all();
    }
}

#[test]
fn the_projects_peek_lists_a_project_read_only_and_imports_a_pick() {
    let mut h = distro();
    // A source project on disk: this project with a sampler track added.
    h.app.graph_controller().add_track(std::path::Path::new(PLATE)).expect("a sampler track");
    let source_track = h.app.tracks.len() - 1;
    let project = h.app.capture_export_project().expect("capture");
    let dir = std::env::temp_dir().join(format!("eseq-peek-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("peek-source.json");
    std::fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
    h.peek_settle();
    let tracks_before = h.app.tracks.len();
    let banks_before = h.app.state.scene_banks().len();

    h.eval_with(
        REFER_PEEK,
        &format!(
            "(let ((v eseq.browser/browser-view)) (set! v.tab \"projects\"))
             (peek-project! \"{}\")",
            path.display()
        ),
    );
    h.peek_settle();
    let (tree, _) = h.buffer_tree("*samples*");
    assert!(widget_keyed(&tree, "project-peek-panel").is_some(), "the peek replaces the list");
    assert!(widget_keyed(&tree, &format!("peek-track-{source_track}")).is_some(), "the track row");
    assert!(widget_keyed(&tree, "project-peek-import-new-bank").is_some());
    assert!(widget_keyed(&tree, "project-peek-import-bank").is_some());
    assert_eq!(h.app.tracks.len(), tracks_before, "peeking changes nothing");

    h.eval_with(
        REFER_PEEK,
        &format!(
            "(toggle-pick! :track {source_track} (get (nth (get (seq-project-peek \"{}\") :tracks) {source_track}) :name))
             (import-picks! :new-bank)",
            path.display()
        ),
    );
    h.peek_settle();
    assert_eq!(h.app.tracks.len(), tracks_before + 1, "the picked track appended");
    assert_eq!(h.app.state.scene_banks().len(), banks_before + 1, "as a new bank");
    let summary = h.eval("(seq-project-import-summary)");
    let Value::Map(summary) = summary else { panic!("a summary map") };
    let lines = summary["lines"].borrow().clone();
    let Value::List(lines) = lines else { panic!("summary lines") };
    assert!(!lines.is_empty());
    let first = lines[0].borrow().clone();
    assert!(
        matches!(&first, Value::String(line) if line.starts_with("Imported 1 track")),
        "{first:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
