//! Isolated process lifecycle and machine-readable progress for the export UI.
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Preparing,
    Rendering { percent: u64 },
    Completed { frames: u64, tail_warning: bool },
    Cancelled,
    Failed { stage: Option<super::ExportStage>, message: String },
}

pub(crate) fn write_status(path: &Path, status: &WorkerStatus) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    serde_json::to_writer(file.as_file_mut(), status)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// A name is a single filename stem, never a relative or absolute path.
pub fn destination(folder: &Path, name: &str) -> io::Result<PathBuf> {
    let name = name.trim();
    let name = name.strip_suffix(".wav").unwrap_or(name);
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a filename without path separators",
        ));
    }
    Ok(folder.join(format!("{name}.wav")))
}

pub fn next_name(folder: &Path, project: &str) -> io::Result<String> {
    let stem: String = project
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = if stem.trim().is_empty() {
        "Untitled"
    } else {
        stem.trim()
    };
    for number in 1..=u32::MAX {
        let name = format!("{stem} ({number})");
        if !destination(folder, &name)?.try_exists()? {
            return Ok(name);
        }
    }
    Err(io::Error::other("No available export filename"))
}

pub struct ExportJobSettings {
    pub destination: PathBuf,
    pub sample_rate: u32,
    pub tail_seconds: f64,
    pub selection: Option<(f64, f64)>,
}

pub struct ExportJob {
    child: Option<Child>,
    directory: Option<tempfile::TempDir>,
    pub destination: PathBuf,
    pub status: WorkerStatus,
}

impl ExportJob {
    /// The executable supports `export-worker` before starting its live engine.
    /// Own an immutable snapshot so subsequent edits or saves cannot change the export.
    pub fn start(
        executable: &Path,
        project: &crate::project::ProjectFile,
        options: ExportJobSettings,
    ) -> io::Result<Self> {
        if options.destination.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "A recording with this name already exists. Choose another name.",
            ));
        }
        let end = project.arrangement.as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Project has no arrangement"))?
            .end_beat;
        super::BouncePlan::new(options.sample_rate, crate::audio::engine::ENGINE_BLOCK_FRAMES,
            project.bpm, end, options.selection, 0, options.tail_seconds)?;
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("project.json");
        serde_json::to_writer(File::create(&input)?, &project)?;
        let cancel = directory.path().join("cancel");
        let status = directory.path().join("status.json");
        let log = File::create(directory.path().join("worker.log"))?;
        let mut command = Command::new(executable);
        command
            .arg("export-worker")
            .arg("--project")
            .arg(&input)
            .arg("--out")
            .arg(&options.destination)
            .arg("--sample-rate")
            .arg(options.sample_rate.to_string())
            .arg("--tail")
            .arg(options.tail_seconds.to_string())
            .arg("--cancel-file")
            .arg(cancel)
            .arg("--status-file")
            .arg(status)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some((start, end)) = options.selection {
            command
                .arg("--start-beat")
                .arg(start.to_string())
                .arg("--end-beat")
                .arg(end.to_string());
        }
        let child = command.spawn()?;
        Ok(Self {
            child: Some(child),
            directory: Some(directory),
            destination: options.destination,
            status: WorkerStatus::Preparing,
        })
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    pub fn cancel(&self) -> io::Result<()> {
        if let Some(directory) = &self.directory {
            File::create(directory.path().join("cancel"))?;
        }
        Ok(())
    }

    pub fn poll(&mut self) -> io::Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let directory = self.directory.as_ref().unwrap();
        let status_path = directory.path().join("status.json");
        // Atomic replacement means every observed status is a complete document.
        match File::open(&status_path) {
            Ok(file) => self.status = serde_json::from_reader(file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(exit) = child.try_wait()? {
            // The terminal update can race the read above; read again after exit.
            if let Ok(file) = File::open(status_path) {
                self.status = serde_json::from_reader(file)?;
            }
            if !matches!(
                self.status,
                WorkerStatus::Completed { .. }
                    | WorkerStatus::Cancelled
                    | WorkerStatus::Failed { .. }
            ) {
                self.status = WorkerStatus::Failed {
                    stage: None,
                    message: format!("Export worker exited ({exit}) without a result"),
                };
            } else if !exit.success() && matches!(self.status, WorkerStatus::Completed { .. }) {
                self.status = WorkerStatus::Failed {
                    stage: None,
                    message: format!("Export worker failed after writing audio ({exit})"),
                };
            }
            self.child = None;
            self.directory = None;
        }
        Ok(())
    }
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        let _ = self.cancel();
        if let Some(mut child) = self.child.take() {
            let directory = self.directory.take();
            // Keep cancellation/input files alive until the child has unwound its
            // transactional writer. Never block the UI or forcibly kill the writer.
            std::thread::spawn(move || {
                let _ = child.wait();
                drop(directory);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_names_are_numbered_and_cannot_escape_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(next_name(dir.path(), "Song").unwrap(), "Song (1)");
        std::fs::write(destination(dir.path(), "Song (1)").unwrap(), b"existing").unwrap();
        assert_eq!(next_name(dir.path(), "Song").unwrap(), "Song (2)");
        assert_eq!(
            destination(dir.path(), "Mix.wav").unwrap(),
            dir.path().join("Mix.wav")
        );
        for name in ["", "..", "../escape", "/escape", "a\\b", "a:b"] {
            assert!(destination(dir.path(), name).is_err());
        }
    }
    #[test]
    fn failure_status_preserves_stage_and_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let status = WorkerStatus::Failed {
            stage: Some(super::super::ExportStage::Preparation),
            message: "missing convolution IR".into(),
        };
        write_status(&path, &status).unwrap();
        let restored: WorkerStatus = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        assert_eq!(restored, status);
    }

    #[cfg(unix)]
    #[test]
    fn process_job_cancels_and_reaps_without_publishing_output() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let project = folder.path().join("project.json");
        let data = serde_json::json!({
            "version": 10, "name": "Test", "bpm": 120, "current_pattern": 0,
            "reverb": {"size": 0.5, "brightness": 0.5, "replace": 0.5},
            "tracks": [], "patterns": [],
            "arrangement": crate::sequencer::ProjectArrangement::new(0, 4.0),
        });
        serde_json::to_writer(File::create(&project).unwrap(), &data).unwrap();
        let executable = folder.path().join("worker");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cancel-file) shift; cancel="$1" ;;
    --status-file) shift; status="$1" ;;
  esac
  shift
done
count=0
while [ ! -e "$cancel" ] && [ "$count" -lt 200 ]; do
  sleep 0.01
  count=$((count + 1))
done
[ -e "$cancel" ] || exit 3
printf '"Cancelled"' > "$status.next"
mv "$status.next" "$status"
exit 1
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = folder.path().join("out.wav");
        let mut snapshot = crate::project::load_project_from_path(&project).unwrap();
        snapshot.bpm = 137;
        let mut job = ExportJob::start(
            &executable,
            &snapshot,
            ExportJobSettings {
                destination: output.clone(),
                sample_rate: 48_000,
                tail_seconds: 0.0,
                selection: None,
            },
        )
        .unwrap();
        let directory = job.directory.as_ref().unwrap().path().to_owned();
        snapshot.bpm = 150;
        let input = crate::project::load_project_from_path(&directory.join("project.json")).unwrap();
        assert_eq!(input.bpm, 137);
        assert_eq!(snapshot.bpm, 150);
        assert_eq!(crate::project::load_project_from_path(&project).unwrap().bpm, 120);
        job.cancel().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while job.running() && std::time::Instant::now() < deadline {
            job.poll().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!job.running());
        assert_eq!(job.status, WorkerStatus::Cancelled);
        assert!(!output.exists());
        assert!(!directory.exists());
    }
}
