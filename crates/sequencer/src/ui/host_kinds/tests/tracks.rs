//! Tracks (with scenes, banks, transport and selection reads): fields, set paths, identity and the observed gating.

use super::*;

#[test]
fn track_scene_bank_transport_selection_and_device_fields_read_through_kinds() {
    let mut h = Harness::new();
    assert!(h.sync(), "the first sync registers and pushes");
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    let t0 = h.track_id(0);
    h.eval("(def t0 (track 0)) (def t1 (track 1))");
    assert_eq!(h.eval("(first (tracks))"), Value::Instance(t0));
    assert_eq!(h.eval("t0"), Value::Instance(t0));
    assert_eq!(h.eval("t0.index"), Value::Number(0.0));
    assert_eq!(h.eval("t1.index"), Value::Number(1.0));
    assert_eq!(h.eval("t1.name"), s(&h.app.tracks[1]));
    assert_eq!(h.eval("(first t0.color)"), Value::Symbol("rgb".into()));
    let volume = h.shared.state.pattern.track_params[0].get_volume() as f64;
    assert_eq!(h.eval("t0.volume"), Value::Number(volume));
    assert_eq!(h.eval("t0.muted"), Value::Bool(false));
    assert_eq!(h.eval("t0.armed"), Value::Bool(false));
    assert_eq!(h.eval("t0.selected"), Value::Bool(true));
    assert_eq!(h.eval("t1.selected"), Value::Bool(false));
    assert_eq!(h.eval("t0.preset"), s(""));
    let steps = h.shared.state.pattern.track_params[0].get_num_steps();
    assert_eq!(h.eval("t0.num-steps"), Value::Number(steps as f64));
    assert_eq!(h.eval("(len t0.steps)"), Value::Number(steps as f64));
    h.eval("(def s3 (nth t0.steps 3))");
    assert_eq!(h.eval("s3.index"), Value::Number(3.0));
    assert_eq!(h.eval("s3.track"), Value::Instance(t0));
    assert_eq!(h.eval("s3.active"), Value::Bool(false));
    assert_eq!(h.eval("s3.playing"), Value::Bool(false));
    assert_eq!(h.eval("s3.selected"), Value::Bool(false));
    assert_eq!(h.eval("(= s3 (nth t0.steps 3))"), Value::Bool(true));
    // Devices: the instrument at slot -1, then the effects.
    let chain = track_device_chain(&h.app, &h.shared.state, 0);
    assert_eq!(
        h.eval("(len t0.devices)"),
        Value::Number(chain.len() as f64)
    );
    if let Some(first) = chain.first() {
        h.eval("(def d0 (first t0.devices))");
        assert_eq!(h.eval("d0.name"), s(&first.name));
        assert_eq!(h.eval("d0.slot"), Value::Number(first.slot as f64));
        assert_eq!(h.eval("d0.enabled"), Value::Bool(first.enabled));
        assert_eq!(h.eval("d0.track"), Value::Instance(t0));
    }
    // Transport, scenes, banks, selection.
    assert_eq!(h.eval("transport.playing"), Value::Bool(false));
    assert_eq!(h.eval("transport.recording"), Value::Bool(false));
    assert_eq!(h.eval("transport.launch-quantize"), s("off"));
    let scene_count = h.shared.state.scene_count();
    assert!(scene_count >= 1);
    assert_eq!(h.eval("(len (scenes))"), Value::Number(scene_count as f64));
    assert_eq!(h.eval("transport.scene"), h.eval("(first (scenes))"));
    assert_eq!(h.eval("transport.queued"), Value::Nil);
    assert_eq!(h.eval("transport.scene.index"), Value::Number(0.0));
    assert_eq!(h.eval("transport.scene.number"), Value::Number(1.0));
    assert_eq!(h.eval("transport.scene.active"), Value::Bool(true));
    assert_eq!(h.eval("transport.scene.queued"), Value::Bool(false));
    assert_eq!(h.eval("transport.scene.name"), {
        let name = h
            .shared
            .state
            .with_project_scenes(|scenes| scenes.scenes[0].name.clone());
        s(&name)
    });
    if h.eval("(len (banks))") != Value::Number(0.0) {
        h.eval("(def b0 (first (banks)))");
        assert_eq!(h.eval("transport.scene.bank"), h.eval("b0"));
        assert_eq!(h.eval("transport.scene.bank.label"), s("A"));
        assert_eq!(h.eval("b0.index"), Value::Number(0.0));
        assert_eq!(h.eval("b0.playing"), Value::Bool(true));
        assert_eq!(h.eval("(first b0.scenes)"), h.eval("(first (scenes))"));
    }
    assert_eq!(h.eval("selection.track"), Value::Instance(t0));
    // Nothing changed: a second sync pushes nothing.
    assert!(!h.sync(), "pushes only changed values");
}

#[test]
fn bound_volume_and_step_playing_follow_changes_without_re_rendering() {
    let mut h = Harness::new();
    h.sync();
    let renders = Rc::new(std::cell::Cell::new(0u32));
    let counter = renders.clone();
    h.editor
        .runtime_mut()
        .register_native("count-render", move |_args, _ctx| {
            counter.set(counter.get() + 1);
            Ok(Value::Nil)
        });
    h.eval(
        r#"(def t0 (track 0))
           (def s3 (nth t0.steps 3))
           (effect-buffer "*kinds*"
             (do (count-render)
                 (label "x" :active #'t0.volume)))
           (def vol #'t0.volume)
           (def playing3 #'s3.playing)"#,
    );
    h.show_all();
    let rendered = renders.get();
    assert!(rendered >= 1);
    let t0 = h.track_id(0);
    assert!(h.rt().host_field_observed(t0, "volume"));
    // Volume: the model changes, the slot follows on the next sync.
    h.shared.state.pattern.track_params[0].set_volume(0.3);
    assert!(h.sync());
    assert!((h.slot("vol") - 0.3).abs() < 1e-6);
    // Step playing: the playhead lands on step 3, then moves on.
    h.shared.state.transport.track_playheads[0].store(3, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    assert!(h.sync());
    assert_eq!(h.slot("playing3"), 1.0);
    h.shared.state.transport.track_playheads[0].store(4, Ordering::Relaxed);
    assert!(h.sync());
    assert_eq!(h.slot("playing3"), 0.0);
    h.editor.runtime_mut().run_reactive_cycle();
    assert_eq!(
        renders.get(),
        rendered,
        "bindings repaint; the view never re-renders"
    );
    // A by-value reader does re-render.
    h.eval(r#"(effect-buffer "*name*" (do (count-render) (label t0.name)))"#);
    h.show_all();
    let rendered = renders.get();
    let payload = h.eval("(dict :track 0 :name \"Kicks\")");
    h.command("rename-track", payload);
    assert!(h.sync());
    h.editor.runtime_mut().run_reactive_cycle();
    assert_eq!(h.eval("t0.name"), s("Kicks"));
    assert!(
        renders.get() > rendered,
        "a by-value read re-renders on change"
    );
}

#[test]
fn set_paths_change_the_model() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (def s3 (nth t0.steps 3))");
    // Volume.
    h.eval("(set! t0.volume 0.25)");
    h.drain();
    assert!((h.shared.state.pattern.track_params[0].get_volume() - 0.25).abs() < 1e-6);
    h.sync();
    assert_eq!(h.eval("t0.volume"), Value::Number(0.25));
    // Mute.
    h.eval("(toggle! t0.muted)");
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert_eq!(h.eval("t0.muted"), Value::Bool(true));
    h.eval("(set! t0.muted true)");
    h.drain();
    assert!(
        h.shared.state.pattern.track_params[0].is_muted(),
        "already muted: no toggle"
    );
    // Record arm.
    h.eval("(toggle! t0.armed)");
    h.drain();
    assert_eq!(h.shared.record_armed.lock().unwrap().first(), Some(&true));
    assert_eq!(h.eval("t0.armed"), Value::Bool(true));
    // Step active.
    h.eval("(toggle! s3.active)");
    h.drain();
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert_eq!(h.eval("s3.active"), Value::Bool(true));
    // Selection.
    h.eval("(set! selection.track (track 1))");
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 1);
    assert_eq!(
        h.eval("(let ((t (track 1))) t.selected)"),
        Value::Bool(true)
    );
    assert_eq!(h.eval("selection.track"), h.eval("(track 1)"));
    // Transport.
    h.eval("(set! transport.playing true)");
    h.drain();
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    assert_eq!(h.eval("transport.playing"), Value::Bool(true));
    h.eval("(toggle! transport.playing)");
    h.drain();
    assert!(!h.shared.state.transport.playing.load(Ordering::Relaxed));
    // Read-only fields stay read-only.
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER} (set! t0.peak 1)"))
        .expect_err("read-only");
    assert!(
        format!("{error:?}").contains("track.peak is read-only"),
        "{error:?}"
    );
}

#[test]
fn track_add_remove_and_reorder_keep_instance_identity() {
    let mut h = Harness::new();
    h.sync();
    let (a, b) = (h.track_id(0), h.track_id(1));
    h.eval("(def held-b (track 1))");
    h.eval("(len (let ((t (track 1))) t.steps))");
    let b_steps = h.steps_of(b);
    assert!(!b_steps.is_empty());
    // Add a track and move it to the front: the old ones re-key.
    h.app.graph_controller().add_empty_track().expect("add");
    h.app
        .graph_controller()
        .move_appended_track_to(0)
        .expect("move");
    assert_eq!(h.app.tracks.len(), 3);
    h.sync();
    let c = h.track_id(0);
    assert!(c != a && c != b);
    assert_eq!(h.track_id(1), a);
    assert_eq!(h.track_id(2), b);
    assert_eq!(h.eval("held-b"), Value::Instance(b));
    assert_eq!(h.eval("held-b.index"), Value::Number(2.0));
    assert_eq!(h.eval("held-b.key"), h.eval("(list 2)"));
    assert_eq!(h.steps_of(b), b_steps, "steps stay with their track");
    assert_eq!(h.eval("(len (tracks))"), Value::Number(3.0));
    // Remove the first original track: its instance goes stale with its
    // steps; the others keep their ids.
    h.app.graph_controller().delete_track(1).expect("delete");
    h.sync();
    assert!(!h.rt().instance_is_live(a));
    assert_eq!(h.track_id(0), c);
    assert_eq!(h.track_id(1), b);
    assert_eq!(h.eval("held-b.index"), Value::Number(1.0));
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    // A rebuilt track shell (new graph nodes, same registry id) keeps the
    // instance: identity is the registry's TrackId, not a graph node.
    let rebuilt = h.app.track_registry.id_at(1).expect("track id");
    h.app.graph.track_node_ids[1].pan_id += 1000;
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.app.track_registry.id_at(1), Some(rebuilt));
    assert_eq!(h.track_id(1), b);
    // A project load replaces every track, although the new project's
    // registry hands out the same TrackIds again.
    let old_ids = h.app.track_registry.ids().to_vec();
    h.command("new-project", Value::Nil);
    assert!(
        h.app
            .track_registry
            .ids()
            .iter()
            .any(|id| old_ids.contains(id)),
        "ids restart per project: {old_ids:?} then {:?}",
        h.app.track_registry.ids()
    );
    h.sync();
    assert!(!h.rt().instance_is_live(b) && !h.rt().instance_is_live(c));
    assert!(h.steps_of(b).is_empty());
    assert_eq!(h.eval("held-b.name"), s(""), "a stale track reads defaults");
}

#[test]
fn unobserved_live_fields_are_never_computed() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (len t0.steps)");
    // Playback with a moving playhead and live meters, nothing observing.
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    for step in 0..8 {
        h.shared.state.transport.track_playheads[0].store(step, Ordering::Relaxed);
        h.sync();
    }
    h.meters.cached_track_peak_levels = vec![0.5, 0.25];
    h.sync();
    for key in [
        f::STEP_PLAYING,
        f::STEP_ACTIVE,
        f::STEP_SELECTED,
        f::TRACK_PEAK,
        f::TRACK_VOLUME,
        f::TRACK_AUDIBLE,
    ] {
        assert_eq!(h.computed(key), 0, "{key:?} computed while unobserved");
    }
    // Observing one step's playhead computes that step only, on change.
    h.eval("(def s5 (nth t0.steps 5)) (def p5 #'s5.playing)");
    let cold = h.computed(f::STEP_PLAYING);
    assert_eq!(cold, 1, "the binding's seed asks the reader once");
    h.shared.state.transport.track_playheads[0].store(5, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("p5"), 1.0);
    assert_eq!(h.computed(f::STEP_PLAYING), cold + 1);
    h.sync();
    assert_eq!(h.computed(f::STEP_PLAYING), cold + 1, "no change, no work");
    // A meter is read only while observed, from the meter cache.
    h.eval("(def peak0 #'t0.peak)");
    assert_eq!(h.slot("peak0"), 0.5);
    assert!(
        h.frame.host_kinds.wants_peaks() || {
            h.sync();
            h.frame.host_kinds.wants_peaks()
        }
    );
    let before = h.computed(f::TRACK_PEAK);
    h.meters.cached_track_peak_levels = vec![0.75, 0.25];
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::TRACK_PEAK), before + 2);
    assert_eq!(h.slot("peak0"), 0.75);
    h.eval("(set! peak0 nil)");
    let before = h.computed(f::TRACK_PEAK);
    h.sync();
    assert_eq!(h.computed(f::TRACK_PEAK), before);
    assert!(!h.frame.host_kinds.wants_peaks());
}

#[test]
fn reconcile_keeps_results_aligned_when_a_registration_fails() {
    let mut h = Harness::new();
    h.sync();
    let t0 = h.track_id(0);
    let sources = KindsHandles::of(&h.shared);
    let shared = RefCell::new(KindsShared::default());
    let mut pusher = Pusher {
        rt: h.editor.runtime_mut(),
        sources: &sources,
        shared: &shared,
        changed: false,
    };
    // 77 is a live instance already at index 0; 88 and 99 cannot register
    // (no such kind): their slots stay None instead of shifting.
    let mut known = HashMap::from([(77, t0)]);
    let ids = reconcile(&mut pusher, "eseq.kinds:missing", &mut known, &[77, 88, 99]);
    assert_eq!(ids, vec![Some(t0), None, None]);
    assert_eq!(known.len(), 1);
}

#[test]
fn repeated_sets_in_one_frame_never_double_toggle() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (def s3 (nth t0.steps 3))");
    // Bound fields are observed: their cells only move on the next sync,
    // so a value-comparing toggle wrapper would flip twice.
    h.eval("(def m #'t0.muted) (def a #'s3.active) (def p #'transport.playing)");
    h.eval(
        "(do (set! t0.muted true) (set! t0.muted true)
             (toggle! s3.active) (toggle! s3.active)
             (set! transport.playing true) (set! transport.playing true)
             (set! t0.armed true) (toggle! t0.armed)
             (set! transport.recording true) (set! transport.recording true))",
    );
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    assert!(h.shared.recording.load(Ordering::Relaxed));
    // `toggle!` reads the (stale or fresh) arm and sets its negation; the
    // arm is applied at once, so the second call sees it.
    assert_eq!(h.shared.record_armed.lock().unwrap().first(), Some(&false));
    h.sync();
    assert_eq!(h.slot("m"), 1.0);
    assert_eq!(h.slot("a"), 1.0);
    // Setting what already holds is a no-op.
    h.eval("(set! t0.muted true) (set! s3.active true) (set! transport.playing true)");
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    h.eval("(set! t0.muted false) (set! transport.playing false)");
    h.drain();
    assert!(!h.shared.state.pattern.track_params[0].is_muted());
    assert!(!h.shared.state.transport.playing.load(Ordering::Relaxed));
}

#[test]
fn model_fields_are_recomputed_only_when_the_model_revision_moves() {
    let mut h = Harness::new();
    h.sync();
    let base = h.model_syncs();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(
        h.model_syncs(),
        base,
        "an unchanged model is not re-derived"
    );
    // Any counter the model fields derive from brings it back.
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.model_syncs(), base + 1);
    h.sync();
    assert_eq!(h.model_syncs(), base + 1);
    // A recorded edit moves the history revision.
    let payload = h.eval("(dict :track 1 :name \"Bass\")");
    h.command("rename-track", payload);
    h.sync();
    assert_eq!(h.model_syncs(), base + 2);
    assert_eq!(h.eval("(let ((t (track 1))) t.name)"), s("Bass"));
    // Live fields still follow every tick.
    h.eval("(def t0 (track 0)) (def vol #'t0.volume)");
    h.shared.state.pattern.track_params[0].set_volume(0.6);
    h.sync();
    assert!((h.slot("vol") - 0.6).abs() < 1e-6);
    assert_eq!(h.model_syncs(), base + 2);
}

#[test]
fn track_audible_follows_mute_and_another_tracks_solo() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (def t1 (track 1)) (def a0 #'t0.audible) (def a1 #'t1.audible)");
    h.sync();
    assert_eq!(h.slot("a0"), 1.0);
    assert_eq!(h.slot("a1"), 1.0);
    // Track 1 soloed: track 0 is silenced, track 1 stays heard.
    h.shared.state.pattern.track_params[1].set_solo(true);
    h.sync();
    assert_eq!(h.slot("a0"), 0.0, "silenced by track 1's solo");
    assert_eq!(h.slot("a1"), 1.0);
    assert_eq!(h.eval("t0.muted"), Value::Bool(false), "not muted itself");
    h.shared.state.pattern.track_params[1].set_solo(false);
    h.shared.state.pattern.track_params[1].set_mute(true);
    h.sync();
    assert_eq!(h.slot("a0"), 1.0);
    assert_eq!(h.slot("a1"), 0.0, "muted");
}
