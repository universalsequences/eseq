use std::fs;
use std::path::Path;

use sequencer::app_paths::AppPaths;
use sequencer::package_samples::reconcile_app_package_samples;
use sequencer::project::{load_project_from_path, ProjectFile, ProjectTrackKind};
use sequencer::sample_db::SampleDb;
use sequencer::sample_manifest::{read_manifest, SampleManifestLine};

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn factory_samples_install_offline_and_survive_project_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let package = root.join("Resources/packages/universalsequences.factory-samples");
    copy_tree(
        &sequencer::app_paths::app_paths().factory_packages_dir().join("universalsequences.factory-samples"),
        &package,
    );
    let paths = AppPaths::release(
        root.join("MacOS"), root.join("Resources"), root.join("Support"),
        root.join("Caches"), root.join("Lisp"),
    );
    paths.ensure_user_tier().unwrap();
    let origin = "pkg:universalsequences.factory-samples";
    let report = reconcile_app_package_samples(&paths).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.ingested_origins, vec![origin]);
    let manifest = read_manifest(&package.join("samples.jsonl")).unwrap();
    let expected: Vec<_> = manifest.iter().filter_map(|line| match line {
        SampleManifestLine::Sample(sample) => Some(sample),
        _ => None,
    }).collect();
    assert!(!expected.is_empty());
    let db = SampleDb::open(&paths.sample_db_path()).unwrap();
    let rows = db.query(&[], &[], None, false, &[origin]).unwrap();
    assert_eq!(rows.len(), expected.len());
    for tag in ["piano", "808", "acoustic"] {
        let matches = db.query_samples_for_browser_with_origins(&[tag], &[origin], None, 100).unwrap();
        assert!(!matches.is_empty(), "browser must discover {tag}");
        assert!(matches.iter().all(|row| row.available));
    }
    // Every shipped file must decode through the same WAV reader the sampler
    // uses, with finite audible PCM. This includes the MP3 -> store conversion.
    for sample in &expected {
        let path = paths.resolve_sample_ref(Path::new(&format!("samples/{}.wav", sample.hash)));
        let audio = eseqlisp::audio::sample::load_wav_file(&path).unwrap();
        assert!(audio.samples.iter().all(|value| value.is_finite()), "{}", sample.path.as_deref().unwrap());
        let peak = audio.samples.iter().fold(0.0_f32, |peak, value| peak.max(value.abs()));
        assert!(peak > 0.001, "inaudible {}: {peak}", sample.title.as_deref().unwrap());
    }

    let tracks: Vec<_> = expected.iter().enumerate().map(|(index, sample)| serde_json::json!({
        "id": index + 1, "kind": "sampler", "name": sample.title,
        "sample_path": format!("samples/{}.wav", sample.hash),
    })).collect();
    let project: ProjectFile = serde_json::from_value(serde_json::json!({
        "version": sequencer::project::project_file_version(), "name": "Factory samples",
        "bpm": 120, "current_pattern": 0,
        "reverb": {"size": 0.2, "brightness": 0.8, "replace": 0.3},
        "tracks": tracks, "patterns": [],
    })).unwrap();
    let project_path = paths.projects_dir().join("factory.json");
    fs::write(&project_path, serde_json::to_vec(&project).unwrap()).unwrap();
    drop(db);
    let again = reconcile_app_package_samples(&paths).unwrap();
    assert_eq!(again.unchanged_origins, vec![origin]);
    assert!(again.removed_origins.is_empty());
    let reopened = load_project_from_path(&project_path).unwrap();
    for (track, sample) in reopened.tracks.iter().zip(&expected) {
        let ProjectTrackKind::Sampler { sample_path: Some(reference) } = &track.kind else {
            panic!("sample reference lost on project reopen");
        };
        assert_eq!(reference, &format!("samples/{}.wav", sample.hash));
        assert!(paths.resolve_sample_ref(Path::new(reference)).is_file());
    }
    assert_eq!(reopened.tracks.len(), expected.len());

    // User shadowing and its removal must select exactly one package and then
    // restore the bundled catalog without dropping its sample claims.
    let user_package = paths.packages_dir().join("universalsequences.factory-samples");
    copy_tree(&package, &user_package);
    fs::write(user_package.join("samples.jsonl"), "").unwrap();
    assert!(reconcile_app_package_samples(&paths).unwrap().errors.is_empty());
    assert!(SampleDb::open(&paths.sample_db_path()).unwrap()
        .query(&[], &[], None, false, &[origin]).unwrap().is_empty());
    fs::remove_dir_all(&user_package).unwrap();
    let restored = reconcile_app_package_samples(&paths).unwrap();
    assert!(restored.errors.is_empty(), "{:?}", restored.errors);
    assert_eq!(restored.ingested_origins, vec![origin]);
    assert_eq!(SampleDb::open(&paths.sample_db_path()).unwrap()
        .query(&[], &[], None, false, &[origin]).unwrap().len(), expected.len());
}
