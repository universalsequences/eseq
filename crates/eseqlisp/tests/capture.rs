#![cfg(target_os = "macos")]

#[test]
#[ignore = "writes /tmp/eseqlisp-patcher-lexilush.png for visual inspection"]
fn capture_patcher_lexilush_png() {
    let out = std::env::temp_dir().join("eseqlisp-patcher-lexilush.png");
    let exe = env!("CARGO_BIN_EXE_eseqlisp_capture");
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let dsp_path = workspace_root.join("crates/sequencer/effects/lexilush/dsp.lisp");
    let source = format!(
        r#"(effect
             (patcher
               :intent :effect
               :path "{}"))"#,
        dsp_path.display()
    );
    let status = std::process::Command::new(exe)
        .args([
            "--source", &source, "--width", "2050", "--height", "1218", "--out",
        ])
        .arg(&out)
        .status()
        .expect("run eseqlisp_capture");

    assert!(status.success(), "eseqlisp_capture exited with {status}");
    let metadata = std::fs::metadata(&out).expect("capture PNG metadata");
    assert!(
        metadata.len() > 16 * 1024,
        "capture PNG was unexpectedly small"
    );
    eprintln!("wrote {}", out.display());
}

#[test]
#[ignore = "writes /tmp/eseqlisp-patcher-segmented-simple.png for visual inspection"]
fn capture_patcher_segmented_simple_png() {
    let out = std::env::temp_dir().join("eseqlisp-patcher-segmented-simple.png");
    let dsp = std::env::temp_dir().join("eseqlisp-patcher-segmented-simple-dsp.lisp");
    std::fs::write(
        &dsp,
        r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
        "#,
    )
    .expect("write simple patcher fixture");
    let exe = env!("CARGO_BIN_EXE_eseqlisp_capture");
    let source = format!(
        r#"(effect
             (patcher
               :intent :effect
               :path "{}"))"#,
        dsp.display()
    );
    let status = std::process::Command::new(exe)
        .args([
            "--source",
            &source,
            "--width",
            "900",
            "--height",
            "620",
            "--click",
            "7.65",
            "9.7",
            "--super-y",
            "--out",
        ])
        .arg(&out)
        .status()
        .expect("run eseqlisp_capture");

    assert!(status.success(), "eseqlisp_capture exited with {status}");
    let metadata = std::fs::metadata(&out).expect("capture PNG metadata");
    assert!(
        metadata.len() > 8 * 1024,
        "capture PNG was unexpectedly small"
    );
    eprintln!("wrote {}", out.display());
}
