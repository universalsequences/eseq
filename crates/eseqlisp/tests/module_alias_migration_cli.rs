use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn migration_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eseqlisp_migrate_module_aliases"))
}

#[test]
fn migration_command_requires_explicit_mode_and_dry_run_is_never_silent() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-module-alias-cli-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("external.lisp");
    let original = "(do (seq-apply-fx-layout) (set! current-step 4))\n";
    fs::write(&path, original).unwrap();

    let missing_mode = migration_command().arg("--").arg(&path).output().unwrap();
    assert!(!missing_mode.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    let dry_run = migration_command()
        .arg("--dry-run")
        .arg("--")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let stdout = String::from_utf8(dry_run.stdout).unwrap();
    assert!(stdout.contains("--- "));
    assert!(stdout.contains("-\u{28}do \u{28}seq-apply-fx-layout\u{29}"));
    assert!(stdout.contains("+\u{28}do \u{28}eseq.seq-layout/apply-fx-layout\u{29}"));
    assert!(stdout.contains("no files changed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    let write = migration_command()
        .arg("--write")
        .arg("--")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        write.status.success(),
        "{}",
        String::from_utf8_lossy(&write.stderr)
    );
    let migrated = fs::read_to_string(&path).unwrap();
    assert!(migrated.contains("eseq.seq-layout/apply-fx-layout"));
    assert!(migrated.contains("eseq.seq-core-state/current-step"));

    let second = migration_command()
        .arg("--write")
        .arg("--")
        .arg(&path)
        .output()
        .unwrap();
    assert!(second.status.success());
    assert!(
        String::from_utf8(second.stdout)
            .unwrap()
            .contains("0 replacements in 0 files")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), migrated);
    fs::remove_dir_all(dir).unwrap();
}
