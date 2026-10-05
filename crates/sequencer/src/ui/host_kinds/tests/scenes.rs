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
    let commands = h.editor.drain_host_commands();
    let names: Vec<&str> = commands
        .iter()
        .filter_map(|command| match command {
            HostCommand::Custom { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["clone-pattern"], "one command; no switch first");
    for command in commands {
        if let HostCommand::Custom { name, payload } = command {
            h.command(&name, payload);
        }
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
