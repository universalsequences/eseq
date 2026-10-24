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
    // The import runs a turn after it is asked for (its loading toast
    // draws first).
    h.peek_settle();
    assert_eq!(h.app.tracks.len(), tracks_before, "queued, not run yet");
    h.peek_settle();
    assert_eq!(h.app.tracks.len(), tracks_before + 1, "the picked track appended");
    assert_eq!(h.app.state.scene_banks().len(), banks_before + 1, "as a new bank");
    assert_eq!(
        h.eval("(import eseq.scene-banks :refer (scene-viewed-bank-index)) (scene-viewed-bank-index)"),
        Value::Number(banks_before as f64),
        "the view follows the import to the new bank, where it is on"
    );
    let (tree, _) = h.buffer_tree("*samples*");
    assert!(
        widget_keyed(&tree, &format!("peek-track-{source_track}-imported")).is_some(),
        "the imported track's row is checked"
    );
    assert_eq!(
        h.eval(&format!(
            "(eseq.view-kit/listed? {source_track} (get (seq-project-imported \"{}\") \"track\"))",
            path.display()
        )),
        Value::Bool(true),
        "the session remembers the import"
    );

    // A picked bank comes as a new bank whichever button: only that one shows.
    h.eval_with(
        REFER_PEEK,
        &format!(
            "(let ((b (first (get (seq-project-peek \"{}\") :banks)))) (toggle-pick! :bank (get b :id) (get b :name)))",
            path.display()
        ),
    );
    h.peek_settle();
    let (tree, _) = h.buffer_tree("*samples*");
    assert!(widget_keyed(&tree, "project-peek-import-new-bank").is_some());
    assert!(widget_keyed(&tree, "project-peek-import-bank").is_none(), "no 'into this bank' for a bank pick");
    let _ = std::fs::remove_dir_all(dir);
}

/// "Import into bank" on the viewed (playing) bank of a two-bank project:
/// a picked kit comes in ON there, with its pads shown.
#[test]
fn a_kit_imported_into_the_viewed_bank_is_on_there() {
    let mut h = distro();
    let (group_id, _) = h.app.create_drum_rack_recorded(Some("Src Kit".into())).expect("rack");
    for offset in [0, 6] {
        let track = h.app.graph_controller().add_track(std::path::Path::new(PLATE)).expect("a sampler pad");
        h.app
            .assign_rack_pad_track_recorded(group_id, sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE + offset, track)
            .expect("pad");
        h.app.state.pattern.patterns[track].set_step_active(0, true);
    }
    h.app.convert_rack_to_clips_recorded(group_id).expect("clips");
    let project = h.app.capture_export_project().expect("capture");
    let dir = std::env::temp_dir().join(format!("eseq-peek-kit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("peek-kit-source.json");
    std::fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();

    // The target: two banks, A = scene 0, B = scene 1, viewing A.
    h.command("clone-pattern", Value::Nil);
    h.command("create-scene-bank", Value::Nil);
    let bank_b = h.app.state.scene_banks()[1].id;
    h.app.state.move_scene_to_scene_bank(1, bank_b).expect("move");
    h.eval("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.peek_settle();
    let groups_before: Vec<u64> = h.app.groups.iter().map(|g| g.id).collect();

    h.eval_with(
        REFER_PEEK,
        &format!(
            "(let ((v eseq.browser/browser-view)) (set! v.tab \"projects\"))
             (peek-project! \"{}\")",
            path.display()
        ),
    );
    h.peek_settle();
    h.eval_with(
        REFER_PEEK,
        &format!(
            "(let ((k (first (get (seq-project-peek \"{}\") :kits)))) (toggle-pick! :kit (get k :id) (get k :name)))
             (import-picks! :bank)",
            path.display()
        ),
    );
    h.peek_settle();
    h.peek_settle();
    let new = h
        .app
        .groups
        .iter()
        .map(|g| g.id)
        .find(|id| !groups_before.contains(id))
        .expect("a new kit");
    let scenes = h.app.state.with_project_scenes(|s| {
        (0..s.scene_count())
            .map(|scene| (s.kit_parked_in_scene(scene, new), s.scene_rack_clip(scene, new)))
            .collect::<Vec<_>>()
    });
    let view = h.eval(&format!(
        "(import eseq.kinds :refer (groups)) (import eseq.bank-power :refer (kit-bank-state))
         (import eseq.scene-banks :refer (scene-viewed-bank kit-bank-view))
         (let ((g (first (filter (lambda (g) (= g.gid {new})) (groups)))) (b (scene-viewed-bank)))
           (list b.label (kit-bank-state g b) (kit-bank-view g b) (map (lambda (x) x.label) g.used-banks)
                 (map (lambda (x) x.label) g.parked-banks)))"
    ));
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(
        format!("{view:?}"),
        "(\"A\" :on :shown (\"A\") (\"B\"))",
        "scenes (parked, clip): {scenes:?}"
    );
}


