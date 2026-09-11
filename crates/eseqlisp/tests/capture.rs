#![cfg(target_os = "macos")]

#[test]
fn keyed_image_capture_matches_the_full_frame_on_first_render() {
    let directory = std::env::temp_dir().join(format!("eseq-keyed-capture-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let source_path = directory.join("source.png");
    image::RgbaImage::from_pixel(64, 32, image::Rgba([200, 30, 90, 255])).save(&source_path).unwrap();
    // Run Metal on the executable's main thread, as required by macOS.
    // Siblings position the figure away from both axes. This exercises the
    // GPU readback origin as well as preserving the surrounding layout.
    let source = format!(r#"(effect (v-stack (box :height 2)
        (h-stack :align :start (box :width 3)
            (image :key "figure" :src "{}" :width 8 :height 4 :fit :stretch)
            (box :width 20 :height 6))))"#, source_path.display());
    let capture = |name: &str, key: Option<&str>, hide_status: bool| {
        let path = directory.join(name);
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_eseqlisp_capture"));
        command.args(["--source", &source, "--width", "640", "--height", "480", "--out"])
            .arg(&path);
        if let Some(key) = key { command.args(["--key", key]); }
        if hide_status { command.arg("--hide-status"); }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        image::open(path).unwrap().to_rgba8()
    };
    let cropped = capture("cropped.png", Some("figure"), false);
    let full = capture("full.png", None, false);
    let without_status = capture("without-status.png", None, true);
    assert_ne!(full, without_status, "hiding the mode line must affect rendered pixels");
    assert!(cropped.width() < full.width() && cropped.height() < full.height());
    assert_eq!(*cropped.get_pixel(cropped.width() / 2, cropped.height() / 2), image::Rgba([200, 30, 90, 255]),
        "the first frame must contain the decoded image");
    let first_image_pixel = |image: &image::RgbaImage| image.enumerate_pixels()
        .find(|(_, _, color)| **color == image::Rgba([200, 30, 90, 255]))
        .map(|(x, y, _)| (x, y)).expect("decoded source pixels");
    let (full_x, full_y) = first_image_pixel(&full);
    let (crop_x, crop_y) = first_image_pixel(&cropped);
    let origin = (full_x - crop_x, full_y - crop_y);
    assert!(origin.0 > 0 && origin.1 > 0, "siblings must position the target away from the origin");
    assert_eq!(cropped, image::imageops::crop_imm(&full,
        origin.0, origin.1, cropped.width(), cropped.height()).to_image());
    assert_eq!(cropped, image::imageops::crop_imm(&without_status,
        origin.0, origin.1, cropped.width(), cropped.height()).to_image(),
        "status visibility must not alter the figure's layout or pixels");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "eseq-4tl: writes /tmp/eseqlisp-patcher-lexilush.png for visual inspection"]
fn capture_patcher_lexilush_png() {
    let out = std::env::temp_dir().join("eseqlisp-patcher-lexilush.png");
    let exe = env!("CARGO_BIN_EXE_eseqlisp_capture");
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let dsp_path = workspace_root.join("content/effects/lexilush/dsp.lisp");
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
#[ignore = "eseq-4tl: writes /tmp/eseqlisp-eq8-spectrum.png for visual inspection"]
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
#[ignore = "eseq-4tl: writes /tmp/eseqlisp-patcher-segmented-simple.png for visual inspection"]
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
