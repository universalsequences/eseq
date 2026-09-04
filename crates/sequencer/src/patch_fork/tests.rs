use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

/// The pre-curation factory tree kept as fixtures; these tests need core/triton.
fn instruments_root() -> PathBuf {
    crate::app_paths::app_paths().dev_instrument_fixtures_dir().expect("dev layout has the instrument fixture tree")
}

fn effects_root() -> PathBuf {
    crate::app_paths::app_paths().effects_dir()
}

/// Scratch dir under the system temp root. Tests must never write into the
/// checked-in `instruments/` tree.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "eseq-patch-fork-tests/{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preset_bank_sibling_path_appends_presets_to_the_directory_name() {
    assert_eq!(
        preset_bank_sibling_path(Path::new("instruments/core/triton")),
        Some(PathBuf::from("instruments/core/triton.presets"))
    );
    // A dot in the directory name must not be treated as an extension.
    assert_eq!(
        preset_bank_sibling_path(Path::new("instruments/wips/my.synth")),
        Some(PathBuf::from("instruments/wips/my.synth.presets"))
    );
}

#[test]
fn forking_triton_brings_its_waves_directory_and_stages_the_preset_bank() {
    let scratch = Scratch::new("triton");
    let draft = scratch.path().join("draft");
    let source = instruments_root().join("core/triton");
    assert!(source.is_dir(), "core/triton must exist for this test");

    fork_patch_files(&source, &draft).expect("fork triton");

    assert!(draft.join("dsp.lisp").is_file(), "dsp.lisp must be copied");
    assert!(draft.join("ui.lisp").is_file(), "ui.lisp must be copied");
    assert!(
        draft.join("waves").is_dir(),
        "waves/ asset dir must be copied recursively"
    );
    let source_waves = std::fs::read_dir(source.join("waves"))
        .expect("read source waves")
        .count();
    let draft_waves = std::fs::read_dir(draft.join("waves"))
        .expect("read draft waves")
        .count();
    assert_eq!(
        source_waves, draft_waves,
        "every wave asset must come along"
    );
    assert!(source_waves > 0, "triton is expected to ship wave assets");

    assert!(
        draft.join(STAGED_PRESET_BANK_FILE).is_file(),
        "the sibling core/triton.presets bank must be staged in the draft"
    );
    assert_eq!(
        std::fs::read(draft.join(STAGED_PRESET_BANK_FILE)).expect("read staged bank"),
        std::fs::read(instruments_root().join("core/triton.presets")).expect("read source bank"),
        "staging must copy the bank verbatim; rewriting happens at finalize"
    );
}

#[test]
fn forking_copies_the_authored_layout_sidecar_when_the_source_has_one() {
    let scratch = Scratch::new("sidecar");
    let source = scratch.path().join("src");
    let draft = scratch.path().join("draft");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("dsp.lisp"), "(out 0 1)\n").unwrap();
    std::fs::write(source.join("dsp.layout.json"), "{\"nodes\":[]}").unwrap();
    std::fs::write(
        source.join("instrument.json"),
        "{\"run_mode\":\"free_patch\"}",
    )
    .unwrap();

    fork_patch_files(&source, &draft).expect("fork");

    assert_eq!(
        std::fs::read_to_string(draft.join("dsp.layout.json")).unwrap(),
        "{\"nodes\":[]}",
        "the authored sidecar is what keeps the forked node graph from scrambling"
    );
    assert!(draft.join("instrument.json").is_file());
}

#[test]
fn forking_a_directory_without_dsp_lisp_is_refused() {
    let scratch = Scratch::new("no-dsp");
    let source = scratch.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    let error = fork_patch_files(&source, &scratch.path().join("draft"))
        .expect_err("a patch dir with no dsp.lisp cannot be forked");
    assert!(error.contains("no dsp.lisp"), "unexpected error: {error}");
}

#[test]
fn forking_an_effect_directory_copies_its_ui() {
    let scratch = Scratch::new("effect");
    let draft = scratch.path().join("draft");
    let source = effects_root().join("lexilush");
    assert!(source.is_dir(), "effects/lexilush must exist for this test");

    fork_patch_files(&source, &draft).expect("fork effect");

    assert!(draft.join("dsp.lisp").is_file());
    assert!(
        !draft.join(STAGED_PRESET_BANK_FILE).exists(),
        "effects have no sibling preset banks"
    );
}

#[test]
fn forking_a_flat_legacy_instrument_normalizes_it_into_the_folder_layout() {
    let scratch = Scratch::new("flat");
    let root = scratch.path().join("instruments");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("harp.lisp"), "(out 0 1)\n").unwrap();
    std::fs::write(root.join("harp.layout.json"), "{\"nodes\":[]}").unwrap();
    std::fs::write(root.join("harp.presets"), "{\"engine_name\":\"harp\"}").unwrap();
    let draft = scratch.path().join("draft");

    fork_patch_source(&root.join("harp.lisp"), &draft).expect("fork flat");

    assert_eq!(
        std::fs::read_to_string(draft.join("dsp.lisp")).unwrap(),
        "(out 0 1)\n"
    );
    assert_eq!(
        std::fs::read_to_string(draft.join("dsp.layout.json")).unwrap(),
        "{\"nodes\":[]}"
    );
    assert!(draft.join(STAGED_PRESET_BANK_FILE).is_file());
}

/// Folder instruments (`instruments/flutefab/`) keep their bank *inside* the
/// directory as `.presets`. It rides along in the recursive copy, but
/// `materialize_forked_assets` skips dot-files, so unless it is also staged the
/// fork finalizes with zero presets and no warning.
#[test]
fn forking_stages_and_materializes_an_in_directory_preset_bank() {
    let scratch = Scratch::new("in-dir-bank");
    let source = scratch.path().join("flutefab");
    let draft = scratch.path().join("draft");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("dsp.lisp"), "(out 0 1)\n").unwrap();
    std::fs::write(
        source.join(PRESET_BANK_FILE),
        "{\"version\":1,\"engine_name\":\"flutefab/\",\
         \"source_file\":\"instruments/flutefab/.lisp\",\
         \"presets\":[{\"name\":\"breathy\"}]}",
    )
    .unwrap();

    fork_patch_files(&source, &draft).expect("fork folder instrument");
    assert!(
        draft.join(STAGED_PRESET_BANK_FILE).is_file(),
        "the in-directory bank must be staged under the name finalize looks for"
    );

    let final_dir = scratch.path().join("instruments").join("my-flute");
    std::fs::create_dir_all(&final_dir).unwrap();
    materialize_forked_assets(&draft, &final_dir, "my-flute/").expect("materialize");

    // Where `resolve_instrument_storage_path("my-flute/", "presets")` looks.
    let bank_path = final_dir.join(PRESET_BANK_FILE);
    assert!(
        bank_path.is_file(),
        "the finalized fork must expose its presets where the loader resolves them"
    );
    let bank: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bank_path).unwrap()).unwrap();
    assert_eq!(bank["engine_name"], "my-flute/");
    assert_eq!(bank["presets"][0]["name"], "breathy");
    assert!(
        !final_dir.join(STAGED_PRESET_BANK_FILE).exists(),
        "the staging file is an implementation detail and must not ship"
    );
}

#[test]
fn fork_patch_source_dispatches_folder_style_sources_to_their_directory() {
    let scratch = Scratch::new("dispatch");
    let draft = scratch.path().join("draft");
    let source = instruments_root().join("core/triton/dsp.lisp");

    fork_patch_source(&source, &draft).expect("fork folder-style");

    assert!(draft.join("waves").is_dir());
}

#[test]
fn rewriting_a_preset_bank_repoints_engine_name_and_source_file_only() {
    let raw = std::fs::read_to_string(instruments_root().join("core/triton.presets"))
        .expect("read triton presets");
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(original["engine_name"], "core/triton/");

    let rewritten = rewrite_preset_bank_json(&raw, "wips/my-triton/").expect("rewrite");
    let parsed: serde_json::Value = serde_json::from_str(&rewritten).unwrap();

    assert_eq!(parsed["engine_name"], "wips/my-triton/");
    assert_eq!(parsed["source_file"], "instruments/wips/my-triton/.lisp");
    assert_eq!(parsed["version"], original["version"]);
    assert_eq!(
        parsed["presets"], original["presets"],
        "preset payloads key by param name and must survive untouched"
    );
}

#[test]
fn rewriting_rejects_non_object_banks() {
    assert!(rewrite_preset_bank_json("[]", "x/").is_err());
    assert!(rewrite_preset_bank_json("not json", "x/").is_err());
}

#[test]
fn materializing_moves_assets_out_and_writes_the_rewritten_bank() {
    let scratch = Scratch::new("materialize");
    let draft = scratch.path().join("draft");
    let source = instruments_root().join("core/triton");
    fork_patch_files(&source, &draft).expect("fork triton");
    std::fs::write(
        draft.join("instrument.json"),
        "{\"run_mode\":\"free_patch\"}",
    )
    .unwrap();

    // Finalize writes dsp.lisp / dsp.layout.json / instrument.json itself.
    let final_dir = scratch.path().join("instruments").join("my-triton");
    std::fs::create_dir_all(&final_dir).unwrap();
    std::fs::write(final_dir.join("dsp.lisp"), "(out 0 1)\n").unwrap();
    std::fs::write(final_dir.join("dsp.layout.json"), "{\"authored\":true}").unwrap();

    materialize_forked_assets(&draft, &final_dir, "my-triton/").expect("materialize");

    assert!(final_dir.join("ui.lisp").is_file(), "ui.lisp must land");
    assert!(final_dir.join("waves").is_dir(), "waves/ must land");
    assert_eq!(
        std::fs::read_to_string(final_dir.join("dsp.lisp")).unwrap(),
        "(out 0 1)\n",
        "the compiled emission finalize wrote must not be clobbered by the draft copy"
    );
    assert_eq!(
        std::fs::read_to_string(final_dir.join("dsp.layout.json")).unwrap(),
        "{\"authored\":true}",
        "the layout finalize wrote must not be clobbered"
    );
    assert!(
        !final_dir.join(STAGED_PRESET_BANK_FILE).exists(),
        "the staging file is an implementation detail and must not ship"
    );
    assert!(
        !final_dir.join("instrument.json").exists(),
        "run mode is written by save_instrument_run_mode, not copied from the draft"
    );

    let bank_path = final_dir.join(PRESET_BANK_FILE);
    assert!(
        bank_path.is_file(),
        "the bank lands inside the dir, where the loader resolves '<slug>/' exactly"
    );
    let bank: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bank_path).unwrap()).unwrap();
    assert_eq!(bank["engine_name"], "my-triton/");
    assert_eq!(bank["source_file"], "instruments/my-triton/.lisp");
    assert!(
        bank["presets"].as_array().map(Vec::len).unwrap_or(0) > 0,
        "the forked bank must carry the source's presets"
    );
}

#[test]
fn materializing_a_plain_draft_is_a_no_op() {
    let scratch = Scratch::new("plain");
    let draft = scratch.path().join("draft");
    std::fs::create_dir_all(&draft).unwrap();
    std::fs::write(draft.join("dsp.lisp"), "(out 0 1)\n").unwrap();
    let final_dir = scratch.path().join("out");
    std::fs::create_dir_all(&final_dir).unwrap();

    materialize_forked_assets(&draft, &final_dir, "plain/").expect("materialize");

    assert_eq!(
        std::fs::read_dir(&final_dir).unwrap().count(),
        0,
        "a never-forked draft contributes nothing at finalize"
    );
    assert!(!scratch.path().join("out.presets").exists());
}
