//! Scenes and banks.

use super::*;

#[test]
fn clone_scene_copies_the_clicked_scene_into_its_bank_without_a_view_switch() {
    let mut h = Harness::new();
    h.sync();
    // Scene 2 (index 1) in a second bank, with step 5 of track 0 on; the
    // first scene playing.
    h.command("clone-pattern", Value::Nil);
    h.eval("(let ((t0 (first (tracks)))) (let ((s5 (nth t0.steps 5))) (do (set! s5.active true) nil)))");
    h.drain();
    h.command("create-scene-bank", Value::Nil);
    let bank_b = h.app.state.scene_banks()[1].id.0;
    h.eval(&format!(
        "(host-command \"move-scene-to-scene-bank\" (dict :scene 1 :bank-id {bank_b}))"
    ));
    h.eval("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.drain();
    assert_eq!(h.app.state.current_scene_index(), 0);
    assert!(!h.shared.state.pattern.patterns[0].is_active(5));
    h.sync();

    h.eval("(eseq.kinds/clone-scene! (nth (scenes) 1))");
    let commands = h.custom_commands();
    let names: Vec<&str> = commands.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["clone-pattern"], "one command; no switch first");
    for (name, payload) in commands {
        h.command(&name, payload);
    }
    let banks = h.app.state.scene_banks();
    assert_eq!(h.app.state.scene_count(), 3);
    assert_eq!(
        (banks[0].len, banks[1].len),
        (1, 2),
        "cloned into scene 2's bank"
    );
    assert_eq!(h.app.state.current_scene_index(), 2, "the clone plays");
    assert!(
        h.shared.state.pattern.patterns[0].is_active(5),
        "a copy of scene 2"
    );

    // delete-scene! of another scene keeps the playing one.
    h.eval("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.drain();
    h.sync();
    h.eval("(eseq.kinds/delete-scene! (nth (scenes) 1))");
    h.drain();
    assert_eq!(h.app.state.scene_count(), 2);
    assert_eq!(h.app.state.current_scene_index(), 0, "scene 1 still plays");
    assert!(!h.shared.state.pattern.patterns[0].is_active(5));
}

#[test]
fn banks_follow_bank_edits_with_their_ids_and_names() {
    // The bank structure the transport strip shows (formerly SEQ.scene-banks):
    // creating, renaming, filling, undoing and deleting a bank re-push its
    // fields; `bid` addresses the scene-bank commands, `name` is the bank's
    // own name ("" when it has none), `label` the shown one.
    let mut h = Harness::new();
    h.command("clone-pattern", Value::Nil);
    h.command("clone-pattern", Value::Nil);
    h.sync();
    let bank = |h: &mut Harness, i: usize, field: &str| {
        h.eval(&format!("(let ((b (nth (banks) {i}))) b.{field})"))
    };
    assert_eq!(h.eval("(len (banks))"), Value::Number(1.0));
    let first_bid = h.app.state.scene_banks()[0].id.0;
    assert_eq!(bank(&mut h, 0, "bid"), Value::Number(first_bid as f64));
    assert_eq!(bank(&mut h, 0, "name"), s(""));
    assert_eq!(bank(&mut h, 0, "label"), s("A"));
    assert_eq!(
        h.eval("(len (let ((b (first (banks)))) b.scenes))"),
        Value::Number(3.0)
    );

    h.command("create-scene-bank", Value::Nil);
    h.sync();
    let created = h.app.state.scene_banks()[1].id.0;
    assert_eq!(bank(&mut h, 1, "bid"), Value::Number(created as f64));
    assert_eq!(bank(&mut h, 1, "label"), s("B"));
    let rename = h.eval(&format!("(dict :bank-id {created} :name \"Peak\")"));
    h.command("rename-scene-bank", rename);
    h.sync();
    assert_eq!(bank(&mut h, 1, "name"), s("Peak"));
    assert_eq!(bank(&mut h, 1, "label"), s("B — Peak"));

    let size = |h: &mut Harness, i: usize| {
        h.eval(&format!("(len (let ((b (nth (banks) {i}))) b.scenes))"))
    };
    let mv = h.eval(&format!("(dict :scene 0 :bank-id {created})"));
    h.command("move-scene-to-scene-bank", mv);
    h.sync();
    assert_eq!(
        (size(&mut h, 0), size(&mut h, 1)),
        (Value::Number(2.0), Value::Number(1.0))
    );
    assert!(matches!(
        sequencer::app::edit::undo(&mut h.app),
        sequencer::app::history::HistoryReplay::Applied(_)
    ));
    h.sync();
    assert_eq!(
        (size(&mut h, 0), size(&mut h, 1)),
        (Value::Number(3.0), Value::Number(0.0))
    );
    let delete = h.eval(&format!("(dict :bank-id {created})"));
    h.command("delete-scene-bank", delete);
    h.sync();
    assert_eq!(h.eval("(len (banks))"), Value::Number(1.0));
}
