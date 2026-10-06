//! Views built on the kinds: defwidget state, the mini-DAW example and the step grid.

use super::*;

#[test]
fn a_defwidget_with_instance_state_reads_host_kinds_and_only_repaints() {
    // kind-bindings spec §7.3: `:state (step track)` names host kinds; the
    // shader's fields become uniforms bound to the fields' slots, and a
    // singleton (`transport`) is read without being passed in.
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
        r#"(defwidget kinds-step-cell
             :width 4 :height 2
             :state (step track)
             :shader
             (sdf/fill (sdf/circle (+ 0.2 (* 0.3 step.playing)))
               (rgba track.color (if transport.playing 1 0.5))))
           (def t0 (track 0))
           (def s3 (nth t0.steps 3))
           (effect-buffer "*cells*"
             (do (count-render)
                 (kinds-step-cell :step s3 :track t0)))
           (def cell (kinds-step-cell :step s3 :track t0))"#,
    );
    h.show_all();
    let rendered = renders.get();
    assert!(rendered >= 1);
    let def =
        eseqlisp::widget_render::sdf_widget::sdf_widget_def("kinds-step-cell").expect("registered");
    assert_eq!(
        def.state_uniforms,
        [
            "step.playing",
            "track.color|r",
            "track.color|g",
            "track.color|b",
            "transport.playing"
        ]
    );
    let t0 = h.track_id(0);
    let s3 = h.steps_of(t0)[3];
    // Held by the widget, the live fields are observed, so the host computes them.
    assert!(h.rt().host_field_observed(s3, "playing"));
    assert!(h.rt().host_field_observed(t0, "color"));

    let uniforms = |h: &mut Harness| -> Vec<f32> {
        let Value::Map(map) = h.eval("cell") else {
            panic!("widget map");
        };
        let props: HashMap<String, Value> = map
            .iter()
            .map(|(key, value)| (key.clone(), value.borrow().clone()))
            .collect();
        def.state_uniforms
            .iter()
            .map(|name| {
                eseqlisp::widget_render::get_f32_prop(
                    &props,
                    &eseqlisp::widget_render::sdf_widget::shader_state_prop_name(name),
                    -1.0,
                )
            })
            .collect()
    };
    let Value::List(color) = h.eval("t0.color") else {
        panic!("rgb");
    };
    let color: Vec<f32> = color[1..]
        .iter()
        .map(|component| match &*component.borrow() {
            Value::Number(n) => *n as f32,
            other => panic!("component {other:?}"),
        })
        .collect();
    let before = uniforms(&mut h);
    assert_eq!(before[0], 0.0);
    assert_eq!(&before[1..4], &color[..]);
    assert_eq!(before[4], 0.0);

    // The playhead lands on step 3 while the transport plays.
    h.shared.state.transport.track_playheads[0].store(3, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    assert!(h.sync());
    let after = uniforms(&mut h);
    assert_eq!(after[0], 1.0);
    assert_eq!(after[4], 1.0);
    h.editor.runtime_mut().run_reactive_cycle();
    assert_eq!(
        renders.get(),
        rendered,
        "instance state repaints; the view never re-renders"
    );
}

/// The mini-DAW example (kind-bindings spec §13 stage 6): the `-noui` view
/// built on eseq.kinds, `#'` and defwidget instance state.
const MINI_DAW: &str = include_str!("../../../../../../docs/examples/mini-daw.lisp");

/// Every binding in `tree`: an instance's as (instance id, field) in `out`,
/// any other (a legacy namespace's) as `namespace.field` in `legacy`.
pub(super) fn instance_bindings(
    tree: &Value,
    out: &mut Vec<(InstanceId, String)>,
    legacy: &mut Vec<String>,
) {
    match tree {
        Value::ReactiveRef {
            namespace, field, ..
        } => match namespace.strip_prefix("%instance/") {
            Some(id) => out.push((id.parse().expect("instance id"), field.clone())),
            None => legacy.push(format!("{namespace}.{field}")),
        },
        Value::Map(map) => {
            for value in map.values() {
                instance_bindings(&value.borrow(), out, legacy);
            }
        }
        Value::List(items) => {
            for item in items {
                instance_bindings(&item.borrow(), out, legacy);
            }
        }
        _ => {}
    }
}

/// The factory DAW, synced and rendered.
pub(super) fn distro() -> Harness {
    let mut h = Harness::with_root(UiRoot::Distro);
    h.sync();
    h.show_all();
    h.sync();
    h.show_all();
    h
}

/// Every map in a widget tree that carries `prop` (a depth-first walk).
pub(super) fn widgets_with_prop(tree: &Value, prop: &str, out: &mut Vec<HashMap<String, Value>>) {
    match tree {
        Value::Map(map) => {
            if map.contains_key(prop) {
                out.push(
                    map.iter()
                        .map(|(key, value)| (key.clone(), value.borrow().clone()))
                        .collect(),
                );
            }
            for value in map.values() {
                widgets_with_prop(&value.borrow(), prop, out);
            }
        }
        Value::List(items) => {
            for item in items {
                widgets_with_prop(&item.borrow(), prop, out);
            }
        }
        _ => {}
    }
}

/// `source` without comments: a `;` outside a string starts one, up to the
/// end of its line (string escapes included).
fn strip_lisp_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let (mut in_string, mut escaped, mut in_comment) = (false, false, false);
    for ch in source.chars() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
                out.push(ch);
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == ';' {
            in_comment = true;
            continue;
        }
        out.push(ch);
    }
    out
}

/// Whether `code` has a symbol starting with `prefix` (`SEQ.` matches
/// `SEQ.steps`, not `MY-SEQ.x`).
fn has_symbol_starting_with(code: &str, prefix: &str) -> bool {
    code.match_indices(prefix).any(|(at, _)| {
        code[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !(before.is_alphanumeric() || "-_/.*+!?<>=#'".contains(before)))
    })
}

/// The legacy reactive forms `source` uses (kind-bindings spec §13 stage
/// 8): string-key bindings, legacy namespaces read dotted, `:bindable`,
/// view state outside kinds. A ported view uses none of them.
pub(super) fn legacy_forms(source: &str) -> Vec<&'static str> {
    // Comments and string contents say nothing about the code.
    let mut code = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in strip_lisp_comments(source).chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                code.push(ch);
            }
            continue;
        }
        in_string = ch == '"';
        code.push(ch);
    }
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut found: Vec<&'static str> = [
        "bind-seq",
        "bind-nth",
        "(bind ",
        "bind-graph",
        "reactive-get",
        "reactive-set",
        "reactive-value",
        ":bindable",
        "defstate",
        "(state ",
    ]
    .into_iter()
    .filter(|form| flat.contains(form))
    .collect();
    found.extend(
        [
            "SEQ.", "SEQV.", "RETRO.", "EXPORT.", "AUDIO.", "MIDI.", "AGENT.", "GRAPH.",
        ]
        .into_iter()
        .filter(|namespace| has_symbol_starting_with(&code, namespace)),
    );
    found
}

#[test]
fn mini_daw_example_has_no_string_key_bindings() {
    let code = strip_lisp_comments(MINI_DAW);
    assert!(
        !code.contains("No project on this line;"),
        "strings survive stripping"
    );
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(legacy_forms(MINI_DAW), Vec::<&str>::new(), "mini-daw.lisp");
    for forbidden in ["with-color", "tr-r", "tr-g", "tr-b", "b01", ":key (str "] {
        assert!(
            !flat.contains(forbidden),
            "mini-daw.lisp uses {forbidden:?}"
        );
    }
    assert!(has_symbol_starting_with("(len SEQ.steps)", "SEQ."));
    assert!(!has_symbol_starting_with("(len MY-SEQ.steps)", "SEQ."));
    // Keys: none bound twice, none of the step keys sgi/bind-step-keys binds.
    let bound: Vec<&str> = flat
        .split("(bind-key \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let unique: HashSet<&str> = bound.iter().copied().collect();
    assert_eq!(unique.len(), bound.len(), "a key bound twice: {bound:?}");
    for step_key in ["ESC", "C-a", "s-a", "BS"] {
        assert!(
            !unique.contains(step_key),
            "{step_key} is sgi/bind-step-keys'"
        );
    }
    assert!(flat.contains("(sgi/bind-step-keys)"));
    assert!(flat.contains("(import eseq.kinds :refer ("));
}

impl Harness {
    /// A buffer's widget tree and its revision.
    pub(super) fn buffer_tree(&self, name: &str) -> (Value, u64) {
        let buffer = self
            .editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == name)
            .unwrap_or_else(|| panic!("{name} exists"));
        (
            buffer
                .widget_tree
                .clone()
                .unwrap_or_else(|| panic!("{name} rendered")),
            buffer.widget_tree_revision,
        )
    }

    /// Load the mini-DAW example and render it.
    fn load_mini_daw(&mut self) {
        self.sync();
        self.editor
            .runtime_mut()
            .eval_str(MINI_DAW)
            .unwrap_or_else(|error| panic!("mini-daw.lisp: {error:?}"));
        self.sync();
        self.show_all();
        self.sync();
        self.show_all();
    }
}

#[test]
fn mini_daw_example_renders_through_kinds_and_only_repaints_on_playback() {
    let mut h = Harness::new();
    h.load_mini_daw();
    // Turn a step on through the kinds' :set path.
    h.eval("(let ((t0 (first (tracks)))) (let ((s0 (first t0.steps))) (do (set! s0.active true) nil)))");
    h.drain();
    h.sync();
    h.show_all();
    h.sync();
    h.show_all();

    let step_def =
        eseqlisp::widget_render::sdf_widget::sdf_widget_def("daw-step").expect("daw-step");
    assert_eq!(
        step_def.state_uniforms,
        [
            "seed",
            "step.active",
            "step.selected",
            "step.playing",
            "transport.playing",
            "track.color|r",
            "track.color|g",
            "track.color|b",
        ]
    );
    let active = eseqlisp::widget_render::sdf_widget::shader_state_prop_name("step.active");
    let playing = eseqlisp::widget_render::sdf_widget::shader_state_prop_name("step.playing");
    let (sequencer, revision) = h.buffer_tree("*sequencer*");
    let mut cells = Vec::new();
    widgets_with_prop(&sequencer, &active, &mut cells);
    let shown = match h.eval("(reduce |n t| (+ n (len t.steps)) 0 (tracks))") {
        Value::Number(n) => n as usize,
        other => panic!("step count {other:?}"),
    };
    assert!(shown >= 32, "two 16-step tracks, got {shown}");
    assert_eq!(cells.len(), shown, "one step cell per step instance");
    for cell in &cells {
        assert!(
            matches!(cell.get(&active), Some(Value::ReactiveRef { .. })),
            "step.active is bound to the step instance"
        );
    }
    let first = eseqlisp::widget_render::get_f32_prop(&cells[0], &active, -1.0);
    assert_eq!(first, 1.0, "the step turned on through set! shows");
    // The mixer binds through the track instance: the mute button reads
    // muted and audible, the fader audible.
    let shader_prop = eseqlisp::widget_render::sdf_widget::shader_state_prop_name;
    let mut mutes = Vec::new();
    widgets_with_prop(&sequencer, &shader_prop("track.muted"), &mut mutes);
    assert_eq!(mutes.len(), 2, "a mute button per track");
    let mut audible = Vec::new();
    widgets_with_prop(&sequencer, &shader_prop("track.audible"), &mut audible);
    assert_eq!(audible.len(), 4, "a mute button and a fader per track");
    // The top tile shows the bank and scene pills.
    let (fx, _) = h.buffer_tree("*fx*");
    let mut pills = Vec::new();
    widgets_with_prop(&fx, "queued", &mut pills);
    assert!(pills.len() >= 2, "a bank pill and a scene pill");

    // The playhead lands on step 3 while the transport plays: the cell's
    // binding follows, and the view never re-renders.
    h.shared.state.transport.track_playheads[0].store(3, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    assert!(h.sync());
    h.show_all();
    let cell3 = &cells[3];
    assert_eq!(
        eseqlisp::widget_render::get_f32_prop(cell3, &playing, -1.0),
        1.0
    );
    h.editor.refresh_runtime_side_effects();
    assert_eq!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "playback only repaints"
    );

    // Control: view state read by value (the scene menu opening)
    // re-renders its buffer, which the revision shows.
    let fx_revision = h.buffer_tree("*fx*").1;
    h.eval("(do (set! scene-menu.open true) nil)");
    h.show_all();
    h.editor.refresh_runtime_side_effects();
    assert_ne!(
        h.buffer_tree("*fx*").1,
        fx_revision,
        "a by-value read re-renders"
    );
}

/// Whether any map in `tree` has a string `prop` containing `needle`.
fn tree_has_string_prop(tree: &Value, prop: &str, needle: &str) -> bool {
    let mut found = Vec::new();
    widgets_with_prop(tree, prop, &mut found);
    found
        .iter()
        .any(|map| matches!(map.get(prop), Some(Value::String(s)) if s.contains(needle)))
}

#[test]
fn mini_daw_opens_the_selected_tracks_instrument_panel_in_the_top_tile() {
    let mut h = Harness::new();
    // A third track with an instrument (a new project's tracks have none),
    // selected.
    h.app.graph_controller().add_blank_sampler_track();
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.load_mini_daw();
    // The host publishes the selected track's panel while *fx* is visible,
    // as the tick does there.
    let panel = build_instrument_panel_value(&h.app, 2, &h.shared.selected_steps);
    let rt = h.editor.runtime_mut();
    rt.set_reactive_value_patch("SEQ", "instrument-panel", panel);
    rt.run_reactive_cycle();
    h.show_all();
    assert_eq!(h.eval("(eseq.effects/device-panel nil)"), Value::Nil);
    h.eval("(let ((t2 (nth (tracks) 2))) (do (set! view.open-device (first t2.devices)) nil))");
    h.show_all();
    h.editor.refresh_runtime_side_effects();
    assert_ne!(
        h.eval("(eseq.effects/device-panel view.open-device)"),
        Value::Nil
    );
    assert_ne!(
        h.eval("(eseq.effects/device-panel-body view.open-device)"),
        Value::Nil
    );
    let (fx, _) = h.buffer_tree("*fx*");
    assert!(
        tree_has_string_prop(&fx, "background", "daw-panel-frame"),
        "the framed panel shows"
    );
    assert!(
        tree_has_string_prop(&fx, "debug-name", "synth-wrapper"),
        "the factory synth body shows: {fx:?}"
    );
    let mut pills = Vec::new();
    widgets_with_prop(&fx, "queued", &mut pills);
    assert!(pills.is_empty(), "the panel replaces the scenes");
    // A device on a track that is not selected has no panel.
    h.shared.current_track.store(0, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval("(eseq.effects/device-panel view.open-device)"),
        Value::Nil
    );
}

#[test]
fn importing_step_grid_interactions_binds_no_keys() {
    let bindings = |h: &Harness, key: &str| h.rt().global_key_binding(key);
    let mut h = Harness::new();
    h.eval("(import eseq.step-grid-interactions :as sgi)");
    for key in ["C-a", ".", "s-a", "BS"] {
        assert_eq!(bindings(&h, key), None, "{key} bound by the import");
    }
    assert_eq!(
        h.eval("(module-loaded? \"eseq.sequencer\")"),
        Value::Bool(false)
    );
    h.eval("(eseq.step-grid-interactions/bind-step-keys)");
    for key in ["ESC", "C-a", "s-a", "BS"] {
        assert!(
            bindings(&h, key)
                .is_some_and(|handler| handler.starts_with("eseq.step-grid-interactions/")),
            "{key}: {:?}",
            bindings(&h, key)
        );
    }
    // The DAW root binds the global ones itself.
    let h = Harness::with_root(UiRoot::Distro);
    assert_eq!(
        bindings(&h, "C-a").as_deref(),
        Some("eseq.step-grid-interactions/seq-global-select-all")
    );
    assert_eq!(
        bindings(&h, ".").as_deref(),
        Some("eseq.step-grid-interactions/seq-global-toggle-record")
    );
}
