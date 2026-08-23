use std::fs;
use std::process::Command;

#[test]
fn text_capture_artifacts_are_stable_and_cover_all_fixed_cases() {
    let root = std::env::temp_dir().join(format!("eseqlisp-text-capture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let first = root.join("first");
    let second = root.join("second");
    let binary = env!("CARGO_BIN_EXE_eseqlisp_text_capture");

    for output in [&first, &second] {
        let result = Command::new(binary)
            .args([
                "--name",
                "fontdue-test",
                "--output-dir",
                output.to_str().expect("UTF-8 temporary path"),
            ])
            .output()
            .expect("run text capture harness");
        assert!(
            result.status.success(),
            "capture failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    let relative_metrics = "fontdue-test/metrics.json";
    let relative_png = "fontdue-test/text.png";
    let first_metrics = fs::read(first.join(relative_metrics)).unwrap();
    let second_metrics = fs::read(second.join(relative_metrics)).unwrap();
    let first_png = fs::read(first.join(relative_png)).unwrap();
    let second_png = fs::read(second.join(relative_png)).unwrap();
    assert_eq!(first_metrics, second_metrics);
    assert_eq!(first_png, second_png);
    assert_eq!(&first_png[..8], b"\x89PNG\r\n\x1a\n");

    let json: serde_json::Value = serde_json::from_slice(&first_metrics).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["backend"], "fontdue");
    let measurements = json["measurements"].as_array().unwrap();
    assert_eq!(measurements.len(), 39);
    assert!(measurements.iter().all(|measurement| {
        let advances = measurement["advance_widths"].as_object().unwrap();
        advances.len() == 95 && advances.contains_key("m")
    }));

    fs::remove_dir_all(root).unwrap();
}
