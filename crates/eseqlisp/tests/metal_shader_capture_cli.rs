//! The Metal reference harness must be reproducible for the same reason the
//! WGSL one must (`shader_capture_cli.rs`): the committed `msl-macos-arm64`
//! capture is only a usable reference for the shader-port comparison if a
//! rerun on the same host reproduces it byte for byte.
#![cfg(all(target_os = "macos", feature = "capture-harness"))]

use std::fs;
use std::process::Command;

#[test]
fn metal_capture_artifacts_are_stable_and_cover_every_scene() {
    let root = std::env::temp_dir().join(format!("eseqlisp-metal-capture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let first = root.join("first");
    let second = root.join("second");
    let binary = env!("CARGO_BIN_EXE_eseqlisp_metal_shader_capture");

    for output in [&first, &second] {
        let result = Command::new(binary)
            .args([
                "--name",
                "msl-test",
                "--output-dir",
                output.to_str().expect("UTF-8 temporary path"),
            ])
            .output()
            .expect("run metal capture harness");
        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            if stderr.contains("no Metal device") {
                eprintln!("SKIPPED: no Metal device available on this machine");
                let _ = fs::remove_dir_all(&root);
                return;
            }
            panic!("capture failed:\n{stderr}");
        }
    }

    let first_manifest = fs::read(first.join("msl-test/manifest.json")).unwrap();
    let second_manifest = fs::read(second.join("msl-test/manifest.json")).unwrap();
    assert_eq!(first_manifest, second_manifest);

    let manifest: serde_json::Value = serde_json::from_slice(&first_manifest).unwrap();
    assert_eq!(
        manifest["schema_version"],
        eseqlisp::metal_shader_capture::SCHEMA_VERSION
    );
    assert_eq!(manifest["backend"], "msl");
    let scenes = manifest["scenes"].as_array().unwrap();

    // The two backends must capture the same scene list, or the directories
    // are not comparable and the whole reference set is worthless.
    let expected: Vec<&str> = eseqlisp::metal_shader_capture::scene_names().to_vec();
    assert_eq!(scenes.len(), expected.len());
    for (scene, name) in scenes.iter().zip(expected) {
        assert_eq!(scene.as_str().unwrap(), name);
    }

    for scene in scenes {
        let relative = format!("msl-test/{}.png", scene.as_str().unwrap());
        let first_png = fs::read(first.join(&relative)).unwrap();
        let second_png = fs::read(second.join(&relative)).unwrap();
        assert_eq!(first_png, second_png, "{relative} is not reproducible");
        assert_eq!(&first_png[..8], b"\x89PNG\r\n\x1a\n");
    }

    fs::remove_dir_all(root).unwrap();
}
