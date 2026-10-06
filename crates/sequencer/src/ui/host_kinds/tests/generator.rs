//! Stage 7g-5: generators (tick-mode sequencers) and their marks, the
//! legacy `SEQ.generator-mark-*` fields as kinds.

use super::*;

const REFER_GENERATOR: &str =
    "(import eseq.kinds :refer (project generators generator-of generator-mark-named))";

const JAKI: &str = "alez/jaki:jaki";

impl Harness {
    fn eval_gen(&mut self, code: &str) -> Value {
        let source = format!("{REFER_GENERATOR}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    fn gen_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_gen(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    /// Define a script generator named `name` and sync; binds it in Lisp as
    /// `name` and returns its sequencer id.
    fn script_generator(&mut self, name: &str) -> u64 {
        self.eval(&format!(
            r#"(def-sequencer "{name}" :resolution :16 :tick (gen-mark 1))"#
        ));
        self.sync();
        let found = format!(r#"(first (filter (lambda (g) (= g.name "{name}")) (generators)))"#);
        self.eval_gen(&format!("(def {name} {found})"));
        num(self.eval_gen(&format!("{name}.gid"))) as u64
    }

    fn generator_syncs(&self) -> u64 {
        self.frame.host_kinds.generators.syncs
    }

    fn play_at(&mut self, sample: Option<u64>) {
        let state = &self.shared.state;
        state
            .transport
            .playing
            .store(sample.is_some(), Ordering::Relaxed);
        state.set_audio_rendered_sample(sample.unwrap_or(0));
    }

    /// The legacy `SEQ.<field>` as the legacy publisher leaves it, with a
    /// view reading it so it publishes.
    fn legacy_mark(&mut self, field: &str) -> Value {
        let buffer = format!("*legacy-{field}*");
        self.eval(&format!(
            r#"(effect-buffer "{buffer}" (label (str (reactive-value (bind-seq "{field}")))))"#
        ));
        self.editor.runtime_mut().run_reactive_cycle();
        let state = self.shared.state.clone();
        sync_generator_mark_fields(self.editor.runtime_mut(), &state, &mut HashMap::new());
        self.eval(&format!(r#"(reactive-value (bind-seq "{field}"))"#))
    }
}

fn instance_command(h: &mut Harness, name: &str, entries: Vec<(&str, Value)>) {
    let map = entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
        .collect();
    h.command(name, Value::Map(map));
}

#[test]
fn generator_marks_read_like_the_legacy_fields_and_follow_the_audio_clock() {
    let mut h = Harness::new();
    let gid = h.script_generator("gen");
    assert_eq!(h.eval_gen("(len gen.marks)"), number(0.0));
    let state = h.shared.state.clone();
    // A hit at 1000 and its successor at 13000 (lookahead stamps both
    // ahead of the audio clock); a keyed mark at 1000.
    state.push_generator_mark(gid, "", 1_000, 1.0);
    state.push_generator_mark(gid, "", 13_000, 2.0);
    state.push_generator_mark(gid, "0.1", 1_000, 4.0);
    h.sync();
    assert_eq!(
        h.eval_gen("(map (lambda (m) m.name) gen.marks)"),
        h.eval_gen(r#"(list "" "0.1")"#),
        "a key appears with its first mark, sorted"
    );
    h.eval_gen(r#"(def m (generator-mark-named gen "")) (def k (generator-mark-named gen "0.1"))"#);
    let (m, k) = (h.gen_instance("m"), h.gen_instance("k"));
    assert_eq!(h.eval_gen("m.generator"), h.eval_gen("gen"));
    assert_eq!(h.eval_gen(r#"(generator-mark-named gen "x")"#), Value::Nil);
    // Legacy parity (cold reads): stopped, before the first hit, at it, past
    // the second.
    let legacy = [
        format!("generator-mark-{gid}"),
        format!("generator-mark-{gid}-0.1"),
    ];
    for (sample, expected) in [
        (None, [0.0, 0.0]),
        (Some(999), [0.0, 0.0]),
        (Some(1_000), [1.0, 4.0]),
        (Some(20_000), [2.0, 4.0]),
    ] {
        h.play_at(sample);
        let kinds = [h.eval_gen("m.value"), h.eval_gen("k.value")];
        assert_eq!(kinds, expected.map(number), "{sample:?}");
        for (field, kinds) in legacy.iter().zip(kinds) {
            assert_eq!(h.legacy_mark(field), kinds, "{field} at {sample:?}");
        }
    }

    // Nothing is computed while unobserved, and an idle tick syncs no
    // generator.
    h.play_at(Some(1_000));
    let (syncs, cold) = (h.generator_syncs(), h.computed(f::GENERATOR_MARK_VALUE));
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.computed(f::GENERATOR_MARK_VALUE), cold);
    assert_eq!(h.generator_syncs(), syncs);
    // Observed: pushed as the audio clock reaches the next hit.
    h.eval_gen(r#"(effect-buffer "*marks*" (label (str m.value)))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    assert!(h.rt().host_field_observed(m, "value"));
    assert!(!h.rt().host_field_observed(k, "value"));
    let cold = h.computed(f::GENERATOR_MARK_VALUE);
    h.sync();
    assert_eq!(
        h.computed(f::GENERATOR_MARK_VALUE),
        cold + 1,
        "the observed mark only"
    );
    assert_eq!(h.rt().instance_field(m, "value").ok(), Some(number(1.0)));
    assert!(!h.sync(), "an unmoved mark pushes nothing");
    h.play_at(Some(13_000));
    assert!(h.sync());
    assert_eq!(h.rt().instance_field(m, "value").ok(), Some(number(2.0)));
    h.play_at(None);
    h.sync();
    assert_eq!(
        h.rt().instance_field(m, "value").ok(),
        Some(number(0.0)),
        "0 while stopped"
    );
    // A new key registers its mark and keeps the others' handles.
    state.push_generator_mark(gid, "chord", 500, 9.0);
    h.sync();
    assert_eq!(h.generator_syncs(), syncs + 1);
    assert_eq!(
        h.eval_gen("(map (lambda (m) m.name) gen.marks)"),
        h.eval_gen(r#"(list "" "0.1" "chord")"#)
    );
    assert_eq!(h.gen_instance(r#"(generator-mark-named gen "")"#), m);
    assert_eq!(h.gen_instance(r#"(generator-mark-named gen "0.1")"#), k);
    // A stamp under a known key moves no model sync.
    state.push_generator_mark(gid, "", 30_000, 3.0);
    h.sync();
    assert_eq!(h.generator_syncs(), syncs + 1);
    // Cleared marks (an instance replacement) drop the mark instances.
    state.clear_generator_marks();
    h.sync();
    assert!(!h.rt().instance_is_live(m) && !h.rt().instance_is_live(k));
    assert_eq!(h.eval_gen("(len gen.marks)"), number(0.0));
}

#[test]
fn generators_follow_their_instances_and_go_stale() {
    let mut h = Harness::new();
    h.eval("(import alez.jaki.kind)");
    let before = crate::host_commands::instances::instance_ids(&h.app);
    instance_command(&mut h, "instance-create", vec![("kind", s(JAKI))]);
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created
        .difference(&before)
        .next()
        .expect("an instance was created");
    h.sync();
    h.eval_gen(&format!(
        "(def j (instance-ref {id})) (def g (generator-of j))"
    ));
    let g = h.gen_instance("g");
    assert_eq!(
        h.eval_gen("(list g.gid g.index g.owner)"),
        h.eval_gen(&format!("(list {id} 0 nil)"))
    );
    assert_eq!(h.gen_instance("(generator-of j.id)"), g);
    assert_eq!(h.gen_instance("(first project.generators)"), g);
    // A script generator joins; a graph-mode sequencer is no generator.
    h.script_generator("other");
    let other = h.gen_instance("other");
    assert_eq!(h.eval_gen("(len (generators))"), number(2.0));
    h.eval("(import alez.neural.variable-reset)");
    instance_command(
        &mut h,
        "instance-create",
        vec![("kind", s("alez/neural:neural"))],
    );
    h.sync();
    assert_eq!(h.eval_gen("(len (generators))"), number(2.0));

    h.shared.state.push_generator_mark(id, "chord", 0, 5.0);
    h.sync();
    h.eval_gen(r#"(def chord (generator-mark-named g "chord"))"#);
    let chord = h.gen_instance("chord");
    h.play_at(Some(10));
    assert_eq!(h.eval_gen("chord.value"), number(5.0));
    // Deleting the instance unpublishes its generator: it and its marks go
    // stale; the other generator is kept and moves up.
    instance_command(&mut h, "instance-delete", vec![("id", number(id as f64))]);
    h.sync();
    assert!(!h.rt().instance_is_live(g) && !h.rt().instance_is_live(chord));
    assert_eq!(h.gen_instance("(first (generators))"), other);
    assert_eq!(h.eval_gen("other.index"), number(0.0));
    assert_eq!(h.eval_gen("(generator-of j)"), Value::Nil);
    // A project load replaces every generator.
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(other));
    h.play_at(None);
}

#[test]
fn a_redefined_script_generator_starts_without_its_old_marks() {
    let mut h = Harness::new();
    let gid = h.script_generator("again");
    let state = h.shared.state.clone();
    state.push_generator_mark(gid, "", 0, 7.0);
    state.push_generator_mark(gid, "chord", 0, 3.0);
    h.sync();
    h.eval_gen(r#"(def m (generator-mark-named again "chord"))"#);
    let (g, m) = (h.gen_instance("again"), h.gen_instance("m"));
    // Removing it drops its marks with it.
    h.eval(r#"(seq-unpublish-sequencer "again")"#);
    h.sync();
    assert!(!h.rt().instance_is_live(g) && !h.rt().instance_is_live(m));
    assert!(state.generator_mark_keys().is_empty());
    // Defined again (the same id): no old mark shows.
    assert_eq!(h.script_generator("again"), gid);
    assert_eq!(h.eval_gen("(len again.marks)"), number(0.0));
    h.play_at(Some(10));
    let legacy = h.legacy_mark(&format!("generator-mark-{gid}-chord"));
    assert_eq!(legacy, number(0.0), "the legacy field shows no old mark");
    h.play_at(None);
}
