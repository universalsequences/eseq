//! Stage 7f: the browser, the sound palette, the editor and the app's views
//! (`browser`, `preset-file`, `slot-presets`, `sound-palette`, `sound`,
//! `editor`, `editor-macro`, `editor-asset`, `asset-info`, `learn` and its
//! rows, `retro` and its rows, `song-export`, `settings`, `midi-device`,
//! `agent`, `factory-promote`, `project.name`,
//! `project.audio-workers-options`, `track.instrument-id`).

use super::*;
use sequencer::app::sound_palette::PaletteTarget;
use crate::presented::{
    present_kit_presets, present_promote, present_sound_presets, presented, AssetInfo, EditorAsset,
    LearnPlanParam, PresetFile, PromoteView, RetroItem, RetroView, LEARN_METHODS,
    LEARN_REFINE_MODES,
};

const REFER_7F: &str = "(import eseq.kinds :refer (track tracks project selection browser \
                        sound-palette editor learn retro song-export settings agent \
                        apply-sound! fork-sound! open-sound-palette! close-sound-palette! \
                        learn-method-options learn-refine-mode-options))";

impl Harness {
    fn eval_7f(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7F}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// A singleton's field, as its cell holds it.
    fn single(&self, kind: &str, field: &str) -> Value {
        self.rt()
            .instance_field(self.singleton(kind), field)
            .expect(field)
    }

    fn cell(&self, id: InstanceId, field: &str) -> Value {
        self.rt().instance_field(id, field).expect(field)
    }

    fn view_pushes(&self) -> u64 {
        self.frame.host_kinds.presented.pushes
    }

    fn status_7f(&self) -> String {
        self.editor.minibuffer.clone().unwrap_or_default()
    }

    /// The sound palette publish, as the tick runs it.
    fn publish_palette(&mut self) {
        sync_sound_palette(&self.app, &mut self.frame.sound_palette, false);
    }
}

fn strings_of(value: Value) -> Vec<String> {
    match value {
        Value::List(items) => items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::String(text) => text.clone(),
                other => panic!("not a string: {other:?}"),
            })
            .collect(),
        other => panic!("not a list: {other:?}"),
    }
}

fn map_get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map(|cell| cell.borrow().clone())
            .unwrap_or(Value::Nil),
        other => panic!("not a map: {other:?}"),
    }
}

fn rows_of(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        other => panic!("not a list: {other:?}"),
    }
}

#[test]
fn browser_sidebar_and_track_instruments_follow_the_presented_sidebar() {
    let mut h = Harness::new();
    h.sync();
    // The fields are what the sidebar publisher recorded (`presented`).
    let check = |h: &Harness| {
        let sidebar = presented(|p| p.sidebar.get().clone());
        for (field, value) in [
            ("instrument-kind", s(sidebar.instrument_kind)),
            ("instrument", s(&sidebar.instrument)),
            ("instrument-label", s(&sidebar.instrument_label)),
            ("preset", s(&sidebar.preset)),
            ("sample", s(&sidebar.sample)),
        ] {
            assert_eq!(h.single(BROWSER, field), value, "{field}");
        }
        for (field, values) in [
            ("presets", &sidebar.presets),
            ("user-presets", &sidebar.user_presets),
            ("engines", &sidebar.engines),
        ] {
            assert_eq!(&strings_of(h.single(BROWSER, field)), values, "{field}");
        }
        assert_eq!(
            h.single(BROWSER, "track"),
            Value::Instance(h.track_id(sidebar.track as u64))
        );
        let slots = h.instances(h.single(BROWSER, "rack-slots"));
        assert_eq!(slots.len(), sidebar.slots.len());
    };
    check(&h);
    // The sidebar follows the shown track: the publisher records it, the
    // tick pushes it.
    h.app
        .graph_controller()
        .add_blank_sampler_track()
        .expect("track");
    h.track_names = h.app.tracks.clone();
    h.sync();
    sync_sidebar_browser(&h.app, 2);
    h.sync();
    check(&h);
    assert_eq!(h.single(BROWSER, "track"), Value::Instance(h.track_id(2)));
    assert_eq!(h.single(BROWSER, "instrument-kind"), s("sampler"));
    // Each track's instrument id is the Instruments tab's.
    for index in 0..h.app.tracks.len() {
        assert_eq!(
            h.cell(h.track_id(index as u64), "instrument-id"),
            s(&track_instrument_id(&h.app, index)),
            "track {index}"
        );
    }
    assert_eq!(
        h.cell(h.track_id(2), "instrument-id"),
        s(&crate::browser::builtin_instrument_id("sampler"))
    );
}

#[test]
fn idle_syncs_push_no_view_and_list_nothing() {
    let mut h = Harness::new();
    h.sync();
    h.sync();
    let pushes = h.view_pushes();
    let generations = presented(|p| {
        (
            p.sidebar.generation(),
            p.sound_presets.generation(),
            p.kit_presets.generation(),
        )
    });
    for _ in 0..3 {
        assert!(!h.sync(), "an idle sync changes nothing");
    }
    assert_eq!(h.view_pushes(), pushes, "no area pushed while idle");
    // The kinds never list: the listings are the legacy publisher's.
    assert_eq!(
        presented(|p| (
            p.sidebar.generation(),
            p.sound_presets.generation(),
            p.kit_presets.generation(),
        )),
        generations
    );
    // A republish of the same sidebar moves no area either.
    sync_sidebar_browser(&h.app, 0);
    h.sync();
    assert_eq!(h.view_pushes(), pushes);
    // The live fields cost nothing until observed.
    let before = h.computed(f::BROWSER_PREVIEW_POSITION);
    h.sync();
    assert_eq!(h.computed(f::BROWSER_PREVIEW_POSITION), before);
    h.eval_7f("(def preview #'browser.preview-position)");
    h.sync();
    assert!(h.computed(f::BROWSER_PREVIEW_POSITION) > before);
    assert_eq!(h.slot("preview"), 0.0, "0 while no preview plays");
}

fn preset(file_type: &'static str, name: &str, pads: usize) -> PresetFile {
    PresetFile {
        file_type,
        icon: if file_type == "kit" {
            "sampler"
        } else {
            "piano"
        },
        name: name.to_string(),
        path: format!("/library/{name}.{file_type}"),
        pads,
        author: "me".to_string(),
        tags: vec!["drums".to_string()],
    }
}

#[test]
fn preset_files_follow_their_file_across_listings() {
    let mut h = Harness::new();
    let listed = vec![preset("sound", "a", 0), preset("sound", "b", 0)];
    present_sound_presets(listed.clone());
    present_kit_presets(vec![preset("kit", "k", 8)]);
    h.sync();
    let sounds = h.instances(h.single(BROWSER, "sound-presets"));
    let kits = h.instances(h.single(BROWSER, "kit-presets"));
    assert_eq!(sounds.len(), 2);
    assert_eq!(kits.len(), 1);
    // The fields are the listed files'.
    for (file, id) in listed.iter().zip(&sounds) {
        assert_eq!(h.cell(*id, "name"), s(&file.name));
        assert_eq!(h.cell(*id, "path"), s(&file.path));
        assert_eq!(h.cell(*id, "author"), s(&file.author));
        assert_eq!(h.cell(*id, "type"), s("sound"));
        assert_eq!(h.cell(*id, "icon"), s("piano"));
    }
    assert_eq!(h.cell(kits[0], "pads"), Value::Number(8.0));
    assert_eq!(h.cell(kits[0], "index"), Value::Number(0.0));
    assert_eq!(
        strings_of(h.cell(kits[0], "tags")),
        vec!["drums".to_string()]
    );
    // A re-listing keeps a file's instance (re-keyed) and drops a gone one.
    let (a, b) = (sounds[0], sounds[1]);
    present_sound_presets(vec![preset("sound", "c", 0), preset("sound", "b", 0)]);
    h.sync();
    let sounds = h.instances(h.single(BROWSER, "sound-presets"));
    assert_eq!(sounds[1], b, "b keeps its instance");
    assert_eq!(h.cell(b, "index"), Value::Number(1.0));
    assert!(
        !h.rt().instance_is_live(a),
        "a gone file's instance is dropped"
    );
    assert_eq!(h.cell(sounds[0], "name"), s("c"));
    assert_eq!(h.instances(h.single(BROWSER, "kit-presets")), kits);
}

#[test]
fn sound_palette_lists_the_tracks_sounds_by_patch_id() {
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.single(SOUND_PALETTE, "open"), Value::Bool(false));
    h.eval_7f("(open-sound-palette! (track 0))");
    h.drain();
    h.publish_palette();
    h.sync();
    // The fields are what the palette publisher recorded (`presented`).
    let palette = presented(|p| p.palette.get().clone()).expect("the palette is open");
    assert!(!palette.entries.is_empty(), "a track has a sound");
    assert_eq!(h.single(SOUND_PALETTE, "open"), Value::Bool(true));
    assert_eq!(
        h.single(SOUND_PALETTE, "track"),
        Value::Instance(h.track_id(0))
    );
    let (target, target_id) = match palette.target {
        PaletteTarget::Take(id) => ("take", id.0 as f64),
        PaletteTarget::Pattern(id) => ("pattern", id.0 as f64),
        PaletteTarget::Cell => ("cell", -1.0),
    };
    assert_eq!(h.single(SOUND_PALETTE, "target"), s(target));
    assert_eq!(h.single(SOUND_PALETTE, "target-id"), Value::Number(target_id));
    assert_eq!(
        h.single(SOUND_PALETTE, "instrument"),
        s(&palette.instrument)
    );
    let sounds = h.instances(h.single(SOUND_PALETTE, "sounds"));
    assert_eq!(sounds.len(), palette.entries.len());
    for (entry, id) in palette.entries.iter().zip(&sounds) {
        assert_eq!(h.cell(*id, "patch-id"), Value::Number(entry.patch.0 as f64));
        assert_eq!(h.cell(*id, "name"), s(&entry.name));
        assert_eq!(h.cell(*id, "referents"), s(&entry.referents));
        assert_eq!(h.cell(*id, "current"), Value::Bool(entry.is_current));
        assert_eq!(h.cell(*id, "base"), Value::Bool(entry.is_base));
        assert_eq!(
            h.cell(*id, "glyph-key"),
            s(&sound_glyph_key(palette.track, entry.patch.0))
        );
        let colored = sound_palette_rgb(entry.color).is_some();
        assert_eq!(h.cell(*id, "colored"), Value::Bool(colored));
        assert_eq!(h.cell(*id, "track"), Value::Instance(h.track_id(0)));
    }
    // Renaming goes through the palette's edit (one undo entry), by the
    // track's id and the patch id.
    let sound = sounds[0];
    let before = h.cell(sound, "name");
    h.eval_7f("(def s0 (first sound-palette.sounds))");
    let undo = h.app.history.undo_len();
    h.eval_7f("(set! s0.name \"Bright\")");
    h.drain();
    assert_eq!(h.app.history.undo_len(), undo + 1);
    h.publish_palette();
    h.sync();
    assert_eq!(h.cell(sound, "name"), s("Bright"), "the same instance");
    assert_eq!(
        h.instances(h.single(SOUND_PALETTE, "sounds"))[0],
        sound,
        "a rename keeps the instance"
    );
    // The same name again is no edit.
    h.eval_7f("(set! s0.name \"Bright\")");
    h.drain();
    assert_eq!(h.app.history.undo_len(), undo + 1);
    app::edit::undo(&mut h.app);
    h.publish_palette();
    h.sync();
    assert_eq!(h.cell(sound, "name"), before, "undo restores the name");
    // Closing drops the sounds: a held one goes stale, never another patch.
    h.eval_7f("(close-sound-palette!)");
    h.drain();
    h.publish_palette();
    h.sync();
    assert_eq!(h.single(SOUND_PALETTE, "open"), Value::Bool(false));
    assert_eq!(h.single(SOUND_PALETTE, "track"), Value::Nil);
    assert!(!h.rt().instance_is_live(sound));
}

#[test]
fn editor_fields_follow_the_published_editor_state() {
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.single(EDITOR, "run-mode"), s("instrument"));
    present_editor(h.editor.runtime_mut(), |e| {
        e.mode = "new-instrument".to_string();
        e.surface = "patch".to_string();
        e.buffer = "*draft*".to_string();
        e.error = "Preview compiling...".to_string();
        e.canceling = true;
        e.open_macro = "lfo".to_string();
    });
    h.sync();
    for (field, value) in [
        ("mode", s("new-instrument")),
        ("surface", s("patch")),
        ("buffer", s("*draft*")),
        ("error", s("Preview compiling...")),
        ("canceling", Value::Bool(true)),
        ("open-macro", s("lfo")),
    ] {
        assert_eq!(h.single(EDITOR, field), value, "{field}");
    }
    // The macro sidebar: the patch's macros, then the library's, kept by
    // name.
    let patch = patch_macro_sidebar(vec![
        ("lfo".to_string(), vec!["rate".to_string()], vec![]),
        ("env".to_string(), vec![], vec!["lfo".to_string()]),
    ]);
    present_editor_sidebar(h.editor.runtime_mut(), |sidebar| {
        sidebar.patch_macros = patch;
        sidebar.assets = vec![EditorAsset {
            reference: "tables/saw".to_string(),
            tier: "factory".to_string(),
            source_path: "/factory/tables/saw.tensor".to_string(),
        }];
        sidebar.selected_asset = Some(AssetInfo {
            reference: "tables/saw".to_string(),
            metadata: Some(eseqlisp::editor::AssetMetadata {
                shape: vec![4, 256],
                wave_count: 4,
                set_count: 1,
                ..Default::default()
            }),
        });
    });
    h.sync();
    let macros = h.instances(h.single(EDITOR, "patch-macros"));
    assert_eq!(macros.len(), 2);
    assert_eq!(h.cell(macros[0], "name"), s("lfo"));
    assert_eq!(h.cell(macros[0], "library"), Value::Bool(false));
    assert_eq!(strings_of(h.cell(macros[0], "params")), vec!["rate"]);
    assert_eq!(strings_of(h.cell(macros[1], "calls")), vec!["lfo"]);
    let assets = h.instances(h.single(EDITOR, "assets"));
    assert_eq!(h.cell(assets[0], "reference"), s("tables/saw"));
    assert_eq!(h.cell(assets[0], "tier"), s("factory"));
    let info = h.rt().singleton_instance(ASSET_INFO).unwrap();
    assert_eq!(h.single(EDITOR, "selected-asset"), Value::Instance(info));
    assert_eq!(h.cell(info, "reference"), s("tables/saw"));
    assert_eq!(h.cell(info, "tensor-kind"), s(""));
    assert_eq!(h.cell(info, "waves-per-set"), Value::Number(0.0));
    assert_eq!(h.cell(info, "wave-count"), Value::Number(4.0));
    // A reorder keeps each macro's instance; a removed one goes stale.
    let (lfo, env) = (macros[0], macros[1]);
    let patch = patch_macro_sidebar(vec![("env".to_string(), vec![], vec![])]);
    present_editor_sidebar(h.editor.runtime_mut(), |sidebar| {
        sidebar.patch_macros = patch;
        sidebar.selected_asset = None;
    });
    h.sync();
    assert_eq!(h.instances(h.single(EDITOR, "patch-macros")), vec![env]);
    assert!(!h.rt().instance_is_live(lfo));
    assert_eq!(h.single(EDITOR, "selected-asset"), Value::Nil);
    // The run mode's setter is the draft run-mode command (an edit session
    // owns it: none here, which it reports).
    h.editor.minibuffer = None;
    h.eval_7f("(set! editor.run-mode \"free_patch\")");
    h.drain();
    assert!(
        h.status_7f().contains("No instrument edit session"),
        "{}",
        h.status_7f()
    );
    // The current mode (any case) round-trips with no session: no error.
    h.editor.minibuffer = None;
    h.eval_7f("(set! editor.run-mode editor.run-mode) (set! editor.run-mode \"Instrument\")");
    h.drain();
    assert_eq!(h.status_7f(), "");
    h.sync();
    assert_eq!(
        h.single(EDITOR, "error"),
        s("Preview compiling..."),
        "no error either"
    );
}

#[test]
fn learn_settings_set_through_the_record_under_the_value_rule() {
    let mut h = Harness::new();
    h.sync();
    let record = || presented(|p| p.learn.get().clone());
    assert_eq!(h.single(LEARN, "epochs"), Value::Number(record().epochs));
    assert_eq!(h.single(LEARN, "method"), s(LEARN_METHODS[0]));
    // The option constants are the host's.
    let methods = strings_of(h.eval_7f("learn-method-options"));
    assert_eq!(methods, LEARN_METHODS.map(str::to_string));
    let modes = strings_of(h.eval_7f("learn-refine-mode-options"));
    assert_eq!(modes, LEARN_REFINE_MODES.map(str::to_string));
    h.eval_7f("(set! learn.epochs 500) (set! learn.method \"evolutionary search only\")");
    h.eval_7f("(set! learn.cma-refine-mode \"scalar\") (set! learn.cma-sigma 0.5)");
    h.eval_7f("(set! learn.cma-population 16) (set! learn.pitch-hz 220)");
    h.drain();
    h.sync();
    for (field, value) in [
        ("epochs", Value::Number(500.0)),
        ("method", s("Evolutionary search only")),
        ("cma-refine-mode", s("Scalar")),
        ("cma-sigma", Value::Number(0.5)),
        ("cma-population", Value::Number(16.0)),
        ("pitch-hz", Value::Number(220.0)),
    ] {
        assert_eq!(h.single(LEARN, field), value, "{field}");
        assert_eq!(record().setting(field), Some(value), "the record's {field}");
    }
    // Out of range, of the wrong shape or unknown: an error, no change.
    for (code, message) in [
        ("(set! learn.epochs 0)", "an integer from 1 to 2000"),
        (
            "(set! learn.cma-population 2)",
            "0 (auto) or an integer from 4",
        ),
        ("(set! learn.cma-sigma 0)", "above 0"),
        ("(set! learn.pitch-hz -1)", "a positive number"),
        ("(set! learn.method \"guess\")", "one of"),
    ] {
        h.editor.minibuffer = None;
        h.eval_7f(code);
        h.drain();
        assert!(h.status_7f().contains(message), "{code}: {}", h.status_7f());
    }
    // A value of the wrong type is set!'s own error.
    let source = format!("{REFER_7F}\n(set! learn.epochs 2.5)");
    let error = h.editor.runtime_mut().eval_str(&source).expect_err(":int");
    assert!(format!("{error:?}").contains("is :int"), "{error:?}");
    h.sync();
    assert_eq!(h.single(LEARN, "epochs"), Value::Number(500.0));
    assert_eq!(h.single(LEARN, "cma-population"), Value::Number(16.0));
    // The current value always works, and changes nothing.
    let pushes = h.view_pushes();
    h.eval_7f("(set! learn.epochs learn.epochs) (set! learn.method learn.method)");
    h.drain();
    h.sync();
    assert_eq!(h.view_pushes(), pushes);
    // Progress and the plan's rows follow what the learn job publishes.
    let row = |name: &str, status: &str, reason: &str| LearnPlanParam {
        name: name.to_string(),
        status: status.to_string(),
        reason: reason.to_string(),
    };
    present_learn(h.editor.runtime_mut(), |l| {
        l.plan_params = vec![
            row("cutoff", "learnable", ""),
            row("seed", "frozen", "noise"),
        ];
        l.phase = "training".to_string();
        l.losses = vec![0.5, 0.25];
    });
    h.sync();
    let rows = h.instances(h.single(LEARN, "plan-params"));
    assert_eq!(rows.len(), 2);
    assert_eq!(h.cell(rows[1], "name"), s("seed"));
    assert_eq!(h.cell(rows[1], "status"), s("frozen"));
    assert_eq!(h.cell(rows[1], "reason"), s("noise"));
    assert_eq!(h.single(LEARN, "phase"), s("training"));
    assert_eq!(
        h.single(LEARN, "losses"),
        list_value(record().losses.iter().map(|loss| Value::Number(*loss)))
    );
}

#[test]
fn retro_capture_rows_and_the_live_audition() {
    let mut h = Harness::new();
    h.sync();
    present_retro(h.editor.runtime_mut(), |r| {
        *r = RetroView {
            lanes: vec!["Kick · C3".to_string(), "Snare · D3".to_string()],
            items: vec![
                RetroItem {
                    lane: 1,
                    start: 0.5,
                    end: 0.75,
                },
                RetroItem {
                    lane: 0,
                    start: 1.0,
                    end: 1.25,
                },
            ],
            duration: 4.0,
            truncated: true,
            error: String::new(),
        }
    });
    h.sync();
    let lanes = h.instances(h.single(RETRO, "lanes"));
    let items = h.instances(h.single(RETRO, "items"));
    assert_eq!(h.cell(lanes[1], "label"), s("Snare · D3"));
    assert_eq!(h.cell(items[0], "lane"), Value::Instance(lanes[1]));
    assert_eq!(h.cell(items[1], "start"), Value::Number(1.0));
    assert_eq!(h.single(RETRO, "duration"), Value::Number(4.0));
    assert_eq!(h.single(RETRO, "truncated"), Value::Bool(true));
    // An error a capture command reports (no capture open) shows here.
    h.command("retrospective-detect", Value::Nil);
    h.sync();
    assert_ne!(h.single(RETRO, "error"), s(""));
    // The audition is live: computed only while observed.
    let before = h.computed(f::RETRO_PLAYING);
    h.sync();
    assert_eq!(h.computed(f::RETRO_PLAYING), before);
    h.eval_7f("(def auditioning #'retro.playing)");
    h.sync();
    assert!(h.computed(f::RETRO_PLAYING) > before);
    assert_eq!(h.slot("auditioning"), 0.0);
    // The playhead is where the audition plays, in seconds of the capture
    // (its crop's span); -1 while none plays.
    h.eval_7f("(def audition-head #'retro.playhead)");
    h.sync();
    assert_eq!(h.slot("audition-head"), -1.0);
    let track = h.app.track_registry.id_at(0).unwrap();
    h.app.retrospective.draft = Some(sequencer::app::retrospective::CaptureDraft {
        notes: vec![sequencer::app::retrospective::CapturedNote {
            track,
            transpose: 0.0,
            velocity: 1.0,
            start: 1.0,
            end: 1.1,
        }],
        duration: 4.0,
        truncated: false,
        scene: h.app.state.current_scene_id().unwrap(),
    });
    h.app.audition_retrospective(1.0, 3.0, 1).unwrap();
    h.sync();
    let head = h.slot("audition-head");
    assert!((1.0..3.0).contains(&head), "playhead {head}");
    h.app.state.note_audition.stop();
    h.sync();
    assert_eq!(h.slot("audition-head"), -1.0);
}

#[test]
fn song_export_follows_the_job_status() {
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.single(SONG_EXPORT, "percent"), Value::Number(-1.0));
    crate::host_commands::export::publish_job_status(
        &mut h.editor,
        &sequencer::bounce::job::WorkerStatus::Rendering { percent: 37 },
        true,
    );
    h.sync();
    assert_eq!(h.single(SONG_EXPORT, "busy"), Value::Bool(true));
    assert_eq!(h.single(SONG_EXPORT, "done"), Value::Bool(false));
    assert_eq!(h.single(SONG_EXPORT, "percent"), Value::Number(37.0));
    assert_eq!(h.single(SONG_EXPORT, "message"), s("Exporting audio — 37%"));
    crate::host_commands::export::publish_job_status(
        &mut h.editor,
        &sequencer::bounce::job::WorkerStatus::Completed {
            frames: 1,
            tail_warning: false,
        },
        false,
    );
    h.sync();
    assert_eq!(h.single(SONG_EXPORT, "busy"), Value::Bool(false));
    assert_eq!(h.single(SONG_EXPORT, "done"), Value::Bool(true));
    assert_eq!(h.single(SONG_EXPORT, "message"), s("Export complete."));
}

#[test]
fn factory_promote_follows_the_presented_promotion() {
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.single(FACTORY_PROMOTE, "blocking"), s(""));
    assert_eq!(h.single(FACTORY_PROMOTE, "skipped"), list_value([]));
    present_promote(|view| {
        *view = PromoteView {
            target: "kit".to_string(),
            destination: "content/kits/".to_string(),
            skipped: vec!["pad 'Kick': skipped".to_string()],
            ..PromoteView::default()
        }
    });
    h.sync();
    assert_eq!(h.single(FACTORY_PROMOTE, "target"), s("kit"));
    assert_eq!(h.single(FACTORY_PROMOTE, "destination"), s("content/kits/"));
    assert_eq!(
        strings_of(h.single(FACTORY_PROMOTE, "skipped")),
        vec!["pad 'Kick': skipped".to_string()]
    );
    // A command's error shows in the modal (a commit with nothing open).
    h.command("factory-promote-commit", Value::Nil);
    h.sync();
    assert_eq!(
        h.single(FACTORY_PROMOTE, "error"),
        s("Open a promotion first")
    );
    assert_eq!(h.single(FACTORY_PROMOTE, "target"), s("kit"));
}

#[test]
fn settings_follow_the_audio_and_midi_state() {
    use sequencer::midi_input::service::{Device, Snapshot};
    let mut h = Harness::new();
    crate::host_commands::audio_settings::publish(&mut h.editor, None);
    h.sync();
    let record = presented(|p| p.settings.get().clone());
    assert_eq!(
        h.single(SETTINGS, "audio-workers-choice"),
        s(&record.workers_choice)
    );
    assert_eq!(
        h.single(SETTINGS, "audio-workers-note"),
        s(&record.workers_note)
    );
    let options = record.workers_options.iter().map(|option| s(option));
    assert_eq!(
        h.single(PROJECT, "audio-workers-options"),
        list_value(options)
    );
    // An unknown choice is an error that changes nothing (the note stays):
    // a label not among the options, even one a lenient parse would take.
    let note = h.single(SETTINGS, "audio-workers-note");
    for choice in ["lots", "0", "9999", "auto"] {
        h.editor.minibuffer = None;
        h.eval_7f(&format!(
            "(set! settings.audio-workers-choice \"{choice}\")"
        ));
        h.drain();
        assert!(
            h.status_7f().contains("audio-workers-choice takes one of"),
            "{choice}: {}",
            h.status_7f()
        );
        h.sync();
        assert_eq!(h.single(SETTINGS, "audio-workers-note"), note);
    }
    // MIDI inputs are kept by device id.
    let device = |id: &str, enabled| Device {
        id: id.to_string(),
        name: format!("Keys {id}"),
        enabled,
        connected: true,
        status: "Connected".to_string(),
    };
    crate::midi_dispatch::sync_midi_devices(
        &mut h.editor,
        Snapshot {
            devices: vec![device("a", true), device("b", false)],
            error: String::new(),
        },
    );
    h.sync();
    let devices = h.instances(h.single(SETTINGS, "midi-devices"));
    assert_eq!(h.cell(devices[0], "device-id"), s("a"));
    assert_eq!(h.cell(devices[1], "enabled"), Value::Bool(false));
    assert_eq!(h.cell(devices[1], "name"), s("Keys b"));
    crate::midi_dispatch::sync_midi_devices(
        &mut h.editor,
        Snapshot {
            devices: vec![device("b", true)],
            error: "boom".to_string(),
        },
    );
    h.sync();
    let after = h.instances(h.single(SETTINGS, "midi-devices"));
    assert_eq!(after, vec![devices[1]], "b keeps its instance");
    assert!(!h.rt().instance_is_live(devices[0]));
    assert_eq!(h.cell(devices[1], "index"), Value::Number(0.0));
    assert_eq!(h.single(SETTINGS, "midi-error"), s("boom"));
    // The setter names the input by its id (the service applies it).
    h.eval_7f("(def d0 (first settings.midi-devices))");
    h.eval_7f("(set! d0.enabled false)");
    let payload = h.last_custom("midi-set-enabled");
    assert_eq!(map_get(&payload, "id"), s("b"));
    assert_eq!(map_get(&payload, "enabled"), Value::Bool(false));
}

#[test]
fn project_name_and_agent_generation() {
    let mut h = Harness::new();
    h.sync();
    h.app.current_project_name = Some("Song".to_string());
    h.sync();
    assert_eq!(h.single(PROJECT, "name"), s("Song"));
    h.app.current_project_name = None;
    h.sync();
    assert_eq!(h.single(PROJECT, "name"), s(""));
    present_agent(h.editor.runtime_mut(), 7);
    h.sync();
    assert_eq!(h.single(AGENT, "generation"), Value::Number(7.0));
}

#[test]
fn a_drum_racks_slots_carry_their_presets_and_their_slot_device() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    sync_sidebar_browser(&h.app, 2);
    h.sync();
    let sidebar = presented(|p| p.sidebar.get().clone());
    let slots = h.instances(h.single(BROWSER, "rack-slots"));
    assert_eq!(slots.len(), sidebar.slots.len());
    assert_eq!(slots.len(), 1);
    assert_eq!(h.single(BROWSER, "instrument"), s(&sidebar.instrument));
    for (slot, id) in sidebar.slots.iter().zip(&slots) {
        assert_eq!(h.cell(*id, "index"), Value::Number(slot.slot as f64));
        assert_eq!(h.cell(*id, "instrument"), s(&slot.instrument));
        assert_eq!(h.cell(*id, "instrument-label"), s(&slot.instrument_label));
        assert_eq!(strings_of(h.cell(*id, "presets")), slot.presets);
        assert_eq!(h.cell(*id, "preset"), s(&slot.preset));
    }
    // The slot names its rack slot device (instance refs, not indices).
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    assert_eq!(h.cell(slots[0], "device"), h.eval_all("rs"));
    assert_eq!(h.eval_all("rs.role"), s("rack-slot"));
    // A republish keeps the instance.
    sync_sidebar_browser(&h.app, 2);
    h.sync();
    assert_eq!(h.instances(h.single(BROWSER, "rack-slots")), slots);
    // Showing another track drops the slots.
    sync_sidebar_browser(&h.app, 0);
    h.sync();
    assert!(h.instances(h.single(BROWSER, "rack-slots")).is_empty());
    assert!(!h.rt().instance_is_live(slots[0]));
}

#[test]
fn the_palette_repushes_when_the_variant_tint_moves_not_the_track_tint() {
    use eseqlisp::backend::Color;
    let mut h = Harness::new();
    h.sync();
    h.eval_7f("(open-sound-palette! (track 0))");
    h.drain();
    h.publish_palette();
    h.sync();
    h.sync();
    let pushes = h.view_pushes();
    let original = eseqlisp::theme::current();
    // The sound palette's colors go through the variant tint only.
    let mut theme = original;
    theme.track_tint = Color::rgba(1.0, 0.0, 0.0, 0.2);
    eseqlisp::theme::set_current(theme);
    h.sync();
    assert_eq!(
        h.view_pushes(),
        pushes,
        "a track tint change re-pushes no palette"
    );
    theme.variant_tint = Color::rgba(0.0, 1.0, 0.0, 0.2);
    eseqlisp::theme::set_current(theme);
    h.sync();
    assert_eq!(
        h.view_pushes(),
        pushes + 1,
        "a variant tint change re-pushes it"
    );
    h.sync();
    assert_eq!(h.view_pushes(), pushes + 1);
    eseqlisp::theme::set_current(original);
}

#[test]
fn an_idle_tick_checks_one_instance_per_collection() {
    use sequencer::midi_input::service::{Device, Snapshot};
    let mut h = Harness::new();
    let devices = |count: usize| Snapshot {
        devices: (0..count)
            .map(|index| Device {
                id: format!("dev-{index}"),
                name: format!("Keys {index}"),
                enabled: true,
                connected: true,
                status: "Connected".to_string(),
            })
            .collect(),
        error: String::new(),
    };
    let idle_checks = |h: &mut Harness| {
        h.sync();
        let before = h.frame.host_kinds.presented.liveness_checks;
        h.sync();
        h.frame.host_kinds.presented.liveness_checks - before
    };
    crate::midi_dispatch::sync_midi_devices(&mut h.editor, devices(4));
    let few = idle_checks(&mut h);
    crate::midi_dispatch::sync_midi_devices(&mut h.editor, devices(400));
    let many = idle_checks(&mut h);
    assert_eq!(h.instances(h.single(SETTINGS, "midi-devices")).len(), 400);
    assert_eq!(few, many, "the scan does not grow with the rows");
    assert!(many <= 10, "one per collection: {many}");
}

#[test]
fn large_listings_reconcile_by_key() {
    let mut h = Harness::new();
    let files: Vec<PresetFile> = (0..3000)
        .map(|index| preset("sound", &format!("s{index}"), 0))
        .collect();
    present_sound_presets(files.clone());
    h.sync();
    let sounds = h.instances(h.single(BROWSER, "sound-presets"));
    assert_eq!(sounds.len(), 3000);
    // Reversed, with one file listed twice (kept once, at its first entry):
    // every file keeps its instance.
    let mut reversed: Vec<PresetFile> = files.into_iter().rev().collect();
    reversed.push(reversed[0].clone());
    present_sound_presets(reversed);
    h.sync();
    let after = h.instances(h.single(BROWSER, "sound-presets"));
    assert_eq!(after.len(), 3000);
    assert_eq!(after[0], sounds[2999]);
    assert_eq!(after[2999], sounds[0]);
    assert_eq!(h.cell(after[0], "index"), Value::Number(0.0));
    assert_eq!(h.cell(after[0], "name"), s("s2999"));
}
