//! The capture harness must be reproducible: the committed goldens are only
//! useful as a reference for the later Metal comparison if a rerun on the same
//! host reproduces them byte for byte.
#![cfg(feature = "wgpu")]

use std::fs;
use std::process::Command;

#[test]
fn shader_capture_artifacts_are_stable_and_cover_every_scene() {
    let root = std::env::temp_dir().join(format!("eseqlisp-shader-capture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let first = root.join("first");
    let second = root.join("second");
    let binary = env!("CARGO_BIN_EXE_eseqlisp_shader_capture");

    for output in [&first, &second] {
        let result = Command::new(binary)
            .args([
                "--name",
                "wgsl-test",
                "--output-dir",
                output.to_str().expect("UTF-8 temporary path"),
            ])
            .output()
            .expect("run shader capture harness");
        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            if stderr.contains("no wgpu adapter") {
                eprintln!("SKIPPED: no wgpu adapter available on this machine");
                let _ = fs::remove_dir_all(&root);
                return;
            }
            panic!("capture failed:\n{stderr}");
        }
    }

    let first_manifest = fs::read(first.join("wgsl-test/manifest.json")).unwrap();
    let second_manifest = fs::read(second.join("wgsl-test/manifest.json")).unwrap();
    assert_eq!(first_manifest, second_manifest);

    let manifest: serde_json::Value = serde_json::from_slice(&first_manifest).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["backend"], "wgsl");
    let scenes = manifest["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), eseqlisp::shader_capture::SCENES.len());

    for scene in scenes {
        let relative = format!("wgsl-test/{}.png", scene.as_str().unwrap());
        let first_png = fs::read(first.join(&relative)).unwrap();
        let second_png = fs::read(second.join(&relative)).unwrap();
        assert_eq!(first_png, second_png, "{relative} is not reproducible");
        assert_eq!(&first_png[..8], b"\x89PNG\r\n\x1a\n");
    }

    fs::remove_dir_all(root).unwrap();
}
