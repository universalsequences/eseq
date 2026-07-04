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
            "--source",
            &source,
            "--width",
            "2050",
            "--height",
            "1218",
            "--patcher-fit",
            "--out",
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
#[ignore = "writes /tmp/eseqlisp-eq8-spectrum.png for visual inspection"]
fn capture_eq8_editor_spectrum_png() {
    let out = std::path::PathBuf::from("/tmp/eseqlisp-eq8-spectrum.png");
    let exe = env!("CARGO_BIN_EXE_eseqlisp_capture");
    let source = r#"
      (effect
        (eq8-editor
        :width 84
        :height 18
        :bands (list
          (dict :id 0 :type :highpass :freq 70 :gain 0 :q 0.71 :enabled true :selected false)
          (dict :id 1 :type :bell :freq 180 :gain -2.5 :q 1.2 :enabled true :selected false)
          (dict :id 2 :type :bell :freq 1200 :gain 7.5 :q 2.4 :enabled true :selected true)
          (dict :id 3 :type :bell :freq 4500 :gain -1.0 :q 1.0 :enabled true :selected false)
          (dict :id 4 :type :bell :freq 8000 :gain 0 :q 1.0 :enabled false :selected false)
          (dict :id 5 :type :bell :freq 12000 :gain 0 :q 1.0 :enabled false :selected false)
          (dict :id 6 :type :bell :freq 16000 :gain 0 :q 1.0 :enabled false :selected false)
          (dict :id 7 :type :lowpass :freq 18000 :gain 0 :q 0.71 :enabled false :selected false))
        :selected-band 2
        :source (dict :kind :master)
        :tap-point :post-fx
        :mode :eq
        :fft-size 8192
        :time-slices 128
        :min-db -96
        :max-db 0
        :smoothing 0.65
        :background-color (rgba 0.045 0.048 0.052 1.0)
        :curve-color (rgba 1.0 0.54 0.14 1.0)
        :selected-color (rgba 1.0 0.78 0.18 1.0)
        :spectrum-color (rgba 0.08 0.52 0.54 0.30)
        :spectrum-peak-color (rgba 0.40 0.92 0.86 0.74)))
    "#;
    let status = std::process::Command::new(exe)
        .args([
            "--source",
            source,
            "--width",
            "760",
            "--height",
            "420",
            "--synthetic-spectrogram",
            "--out",
        ])
        .arg(&out)
        .status()
        .expect("run eseqlisp_capture");

    assert!(status.success(), "eseqlisp_capture exited with {status}");
    let metadata = std::fs::metadata(&out).expect("capture PNG metadata");
    assert!(
        metadata.len() > 12 * 1024,
        "capture PNG was unexpectedly small"
    );
    assert_eq8_capture_has_spectrum_pixels(&out);
    eprintln!("wrote {}", out.display());
}

fn assert_eq8_capture_has_spectrum_pixels(path: &std::path::Path) {
    let image = image::ImageReader::open(path)
        .expect("open EQ8 capture PNG")
        .decode()
        .expect("decode EQ8 capture PNG")
        .to_rgba8();
    let mut teal_pixels = 0usize;
    let mut orange_pixels = 0usize;
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        if a > 20 && g > 70 && b > 65 && (g as f32) > (r as f32) * 1.15 && (b as f32) > r as f32 {
            teal_pixels += 1;
        }
        if a > 20 && r > 150 && g > 55 && g < 190 && b < 110 {
            orange_pixels += 1;
        }
    }
    assert!(
        teal_pixels > 500,
        "expected visible EQ8 spectrum pixels, found {teal_pixels}"
    );
    assert!(
        orange_pixels > 200,
        "expected visible EQ8 response pixels, found {orange_pixels}"
    );
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
