//! Durable host for `DGenLisp train` patch-learning jobs.
//!
//! Each launch snapshots every input before spawning the trainer, appends the
//! validated NDJSON stream to `events.jsonl`, and reports lines and process
//! termination over a non-blocking channel. Persisted snapshots can be loaded
//! after an app restart without depending on the lifetime of an in-memory UI
//! session.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_JOB_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrozenParam {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDelta {
    pub from: f64,
    pub to: f64,
}

/// The stable part of the trainer event protocol. Unknown event kinds and
/// additional fields are retained, allowing an older host to preserve and
/// replay output from a newer trainer rather than corrupting the job log.
#[derive(Clone, Debug, PartialEq)]
pub enum LearnEvent {
    Plan {
        learnable: Vec<String>,
        frozen: Vec<FrozenParam>,
        unsupported: Vec<Value>,
        seed_echo: BTreeMap<String, f64>,
        pitch_hz: f64,
        gate_frames: u64,
        crop_frames: u64,
    },
    Stage {
        name: String,
        total: u64,
    },
    Epoch {
        epoch: u64,
        total: u64,
        loss: f64,
        params: BTreeMap<String, f64>,
        steps: BTreeMap<String, f64>,
    },
    Checkpoint {
        epoch: u64,
        wav: PathBuf,
    },
    Result {
        improvement_pct: f64,
        abs_distance: f64,
        basin_check: String,
        deltas: BTreeMap<String, ParamDelta>,
        final_wav: PathBuf,
        raw: Value,
    },
    Error {
        message: String,
    },
    Unknown(Value),
}

impl LearnEvent {
    pub fn parse(line: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid trainer JSON: {error}"))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "trainer event is missing string field 'type'".to_string())?;
        match kind {
            "plan" => Ok(Self::Plan {
                learnable: string_array(&value, "learnable")?,
                frozen: parse_field(&value, "frozen")?,
                unsupported: value
                    .get("unsupported")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| "plan event is missing array field 'unsupported'".to_string())?,
                seed_echo: parse_field(&value, "seed_echo")?,
                pitch_hz: number(&value, "pitch_hz")?,
                gate_frames: unsigned(&value, "gate_frames")?,
                crop_frames: unsigned(&value, "crop_frames")?,
            }),
            "stage" => Ok(Self::Stage {
                name: string(&value, "name")?,
                total: unsigned(&value, "total")?,
            }),
            "epoch" => Ok(Self::Epoch {
                epoch: unsigned(&value, "epoch")?,
                total: unsigned(&value, "total")?,
                loss: number(&value, "loss")?,
                params: parse_field(&value, "params")?,
                steps: match value.get("steps") {
                    Some(steps) => serde_json::from_value(steps.clone())
                        .map_err(|error| format!("epoch field 'steps' has invalid values: {error}"))?,
                    None => BTreeMap::new(),
                },
            }),
            "checkpoint" => Ok(Self::Checkpoint {
                epoch: unsigned(&value, "epoch")?,
                wav: PathBuf::from(string(&value, "wav")?),
            }),
            "result" => Ok(Self::Result {
                improvement_pct: number(&value, "improvement_pct")?,
                abs_distance: number(&value, "abs_distance")?,
                basin_check: string(&value, "basin_check")?,
                deltas: parse_field(&value, "deltas")?,
                final_wav: PathBuf::from(string(&value, "final_wav")?),
                raw: value,
            }),
            "error" => Ok(Self::Error {
                message: string(&value, "message")?,
            }),
            _ => Ok(Self::Unknown(value)),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Result { .. } | Self::Error { .. })
    }
}

fn parse_field<T: for<'de> Deserialize<'de>>(value: &Value, key: &str) -> Result<T, String> {
    let field = value
        .get(key)
        .ok_or_else(|| format!("trainer event is missing field '{key}'"))?;
    serde_json::from_value(field.clone())
        .map_err(|error| format!("trainer event field '{key}' is invalid: {error}"))
}

fn string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("trainer event is missing string field '{key}'"))
}

fn string_array(value: &Value, key: &str) -> Result<Vec<String>, String> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("trainer event is missing array field '{key}'"))?;
    values
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("trainer event field '{key}' contains a non-string"))
        })
        .collect()
}

fn number(value: &Value, key: &str) -> Result<f64, String> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("trainer event is missing numeric field '{key}'"))
}

fn unsigned(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("trainer event is missing unsigned field '{key}'"))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearnSeed {
    pub params: BTreeMap<String, f64>,
}

#[derive(Clone, Debug)]
pub struct LearnJobSpec {
    pub patch_path: PathBuf,
    /// Exact editor revision to train. The path is retained for provenance
    /// and asset resolution, but this source is the authoritative snapshot.
    pub patch_source: String,
    pub target_path: PathBuf,
    pub seed: LearnSeed,
    pub epochs: u64,
    pub pitch_hz: Option<f64>,
    pub gate_frames: Option<u64>,
    pub plan_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedLearnRequest {
    pub job_id: String,
    pub created_unix_ms: u128,
    pub argv: Vec<String>,
    pub patch_path: PathBuf,
    pub target_path: PathBuf,
    pub epochs: u64,
    pub pitch_hz: Option<f64>,
    pub gate_frames: Option<u64>,
    pub plan_only: bool,
}

#[derive(Debug)]
pub enum LearnJobUpdate {
    Event(LearnEvent),
    ProtocolError { line: String, error: String },
    Exited { success: bool, code: Option<i32> },
    IoError(String),
}

#[derive(Clone, Debug)]
pub struct LearnJobLauncher {
    tool_path: PathBuf,
    jobs_root: PathBuf,
}

impl LearnJobLauncher {
    pub fn from_app_paths(paths: &crate::app_paths::AppPaths) -> Self {
        Self::new(paths.dgenlisp_tool(), paths.learn_jobs_dir())
    }

    pub fn new(tool_path: PathBuf, jobs_root: PathBuf) -> Self {
        Self { tool_path, jobs_root }
    }

    pub fn launch(&self, spec: LearnJobSpec) -> Result<LearnJob, String> {
        validate_spec(&spec)?;
        let (job_id, job_dir) = create_job_dir(&self.jobs_root)?;
        let patch_snapshot = job_dir.join("patch.lisp");
        let seed_snapshot = job_dir.join("seed.json");
        let events_path = job_dir.join("events.jsonl");
        let stderr_path = job_dir.join("stderr.log");

        fs::write(&patch_snapshot, spec.patch_source.as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", patch_snapshot.display()))?;
        snapshot_patch_assets(&spec, &job_dir)?;
        write_json(&seed_snapshot, &spec.seed)?;
        File::create(&events_path)
            .map_err(|error| format!("failed to create {}: {error}", events_path.display()))?;
        let stderr_file = File::create(&stderr_path)
            .map_err(|error| format!("failed to create {}: {error}", stderr_path.display()))?;

        let argv = build_argv(&patch_snapshot, &spec, &seed_snapshot, &job_dir);
        let request = PersistedLearnRequest {
            job_id: job_id.clone(),
            created_unix_ms: unix_ms()?,
            argv: argv.clone(),
            patch_path: spec.patch_path,
            target_path: spec.target_path,
            epochs: spec.epochs,
            pitch_hz: spec.pitch_hz,
            gate_frames: spec.gate_frames,
            plan_only: spec.plan_only,
        };
        write_json(&job_dir.join("request.json"), &request)?;

        let mut child = Command::new(&self.tool_path)
            .args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "failed to launch patch-learning tool {}: {error}",
                    self.tool_path.display()
                )
            })?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "patch-learning child stdout was not piped".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "patch-learning child stderr was not piped".to_string())?;
        fs::write(job_dir.join("pid"), format!("{pid}\n"))
            .map_err(|error| format!("failed to persist patch-learning pid: {error}"))?;

        let (sender, receiver) = mpsc::channel();
        let stdout_thread = spawn_stdout_reader(stdout, events_path.clone(), sender.clone());
        let stderr_thread = spawn_stderr_reader(stderr, stderr_file, sender.clone());
        std::thread::spawn(move || {
            match child.wait() {
                Ok(status) => {
                    // `wait` may observe process exit before the pipe readers
                    // consume their final buffered lines. Join them first so
                    // `Exited` is an actual end-of-stream marker for the UI.
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    let _ = sender.send(exit_update(status));
                }
                Err(error) => {
                    let _ = sender.send(LearnJobUpdate::IoError(format!(
                        "failed waiting for patch-learning process: {error}"
                    )));
                }
            }
        });

        Ok(LearnJob {
            job_id,
            job_dir,
            pid,
            receiver,
        })
    }

    pub fn load(&self, job_id: &str) -> Result<LearnJobSnapshot, String> {
        validate_job_id(job_id)?;
        LearnJobSnapshot::load(self.jobs_root.join(job_id))
    }

    pub fn list(&self) -> Result<Vec<LearnJobSnapshot>, String> {
        let entries = match fs::read_dir(&self.jobs_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "failed to read learn jobs at {}: {error}",
                    self.jobs_root.display()
                ))
            }
        };
        let mut snapshots = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("failed to read learn-job entry: {error}"))?;
            if !entry.file_type().map_err(|error| error.to_string())?.is_dir() {
                continue;
            }
            if let Ok(snapshot) = LearnJobSnapshot::load(entry.path()) {
                snapshots.push(snapshot);
            }
        }
        snapshots.sort_by_key(|snapshot| snapshot.request.created_unix_ms);
        Ok(snapshots)
    }
}

#[derive(Debug)]
pub struct LearnJob {
    pub job_id: String,
    pub job_dir: PathBuf,
    pid: u32,
    pub receiver: Receiver<LearnJobUpdate>,
}

impl LearnJob {
    /// Request graceful cancellation. The waiter still publishes `Exited`, so
    /// callers can keep the handle until the process has actually stopped.
    #[cfg(unix)]
    pub fn cancel(&self) -> Result<(), String> {
        let result = unsafe { libc::kill(self.pid as i32, libc::SIGTERM) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "failed to send SIGTERM to patch-learning process {}: {}",
                self.pid,
                std::io::Error::last_os_error()
            ))
        }
    }

    #[cfg(not(unix))]
    pub fn cancel(&self) -> Result<(), String> {
        Err("graceful patch-learning cancellation is not implemented on this platform".to_string())
    }
}

#[derive(Clone, Debug)]
pub struct LearnJobSnapshot {
    pub job_dir: PathBuf,
    pub request: PersistedLearnRequest,
    pub seed: LearnSeed,
    pub events: Vec<LearnEvent>,
    pub protocol_errors: Vec<String>,
}

impl LearnJobSnapshot {
    pub fn load(job_dir: PathBuf) -> Result<Self, String> {
        let request = read_json(&job_dir.join("request.json"))?;
        let seed = read_json(&job_dir.join("seed.json"))?;
        let mut events = Vec::new();
        let mut protocol_errors = Vec::new();
        let file = File::open(job_dir.join("events.jsonl"))
            .map_err(|error| format!("failed to open persisted learn events: {error}"))?;
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|error| format!("failed reading persisted learn events: {error}"))?;
            match LearnEvent::parse(&line) {
                Ok(event) => events.push(event),
                Err(error) => protocol_errors.push(format!("line {}: {error}", index + 1)),
            }
        }
        Ok(Self {
            job_dir,
            request,
            seed,
            events,
            protocol_errors,
        })
    }

    pub fn terminal_event(&self) -> Option<&LearnEvent> {
        self.events.iter().rev().find(|event| event.is_terminal())
    }
}

fn validate_spec(spec: &LearnJobSpec) -> Result<(), String> {
    if !spec.patch_path.is_file() {
        return Err(format!("patch does not exist: {}", spec.patch_path.display()));
    }
    if !spec.target_path.is_file() {
        return Err(format!("target sample does not exist: {}", spec.target_path.display()));
    }
    if spec.epochs == 0 {
        return Err("epoch count must be greater than zero".to_string());
    }
    if matches!(spec.pitch_hz, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("pitch override must be finite and greater than zero".to_string());
    }
    if matches!(spec.gate_frames, Some(0)) {
        return Err("gate frames override must be greater than zero".to_string());
    }
    Ok(())
}

fn snapshot_patch_assets(spec: &LearnJobSpec, job_dir: &Path) -> Result<(), String> {
    let source_dir = spec
        .patch_path
        .parent()
        .ok_or_else(|| format!("patch has no parent directory: {}", spec.patch_path.display()))?;
    for reference in crate::lisp_host::dylib_cache::asset_references(&spec.patch_source)? {
        let relative = Path::new(&reference);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "patch-learning asset path must stay inside the patch directory: {reference}"
            ));
        }
        let source = source_dir.join(relative);
        if !source.is_file() {
            return Err(format!(
                "patch-learning asset does not exist: {}",
                source.display()
            ));
        }
        let destination = job_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("failed to create asset snapshot directory {}: {error}", parent.display())
            })?;
        }
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "failed to snapshot patch-learning asset {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn build_argv(
    patch_snapshot: &Path,
    spec: &LearnJobSpec,
    seed_snapshot: &Path,
    job_dir: &Path,
) -> Vec<String> {
    let mut argv = vec![
        "train".to_string(),
        "--patch".to_string(),
        patch_snapshot.to_string_lossy().into_owned(),
        "--target".to_string(),
        spec.target_path.to_string_lossy().into_owned(),
        "--seed-params".to_string(),
        seed_snapshot.to_string_lossy().into_owned(),
        "--job-dir".to_string(),
        job_dir.to_string_lossy().into_owned(),
        "--mode".to_string(),
        "direction".to_string(),
        "--epochs".to_string(),
        spec.epochs.to_string(),
    ];
    if let Some(gate_frames) = spec.gate_frames {
        argv.extend(["--gate-frames".to_string(), gate_frames.to_string()]);
    }
    if let Some(pitch_hz) = spec.pitch_hz {
        argv.extend(["--pitch-hz".to_string(), pitch_hz.to_string()]);
    }
    if spec.plan_only {
        argv.push("--plan-only".to_string());
    }
    argv
}

fn spawn_stdout_reader(
    stdout: impl std::io::Read + Send + 'static,
    events_path: PathBuf,
    sender: Sender<LearnJobUpdate>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let file = match OpenOptions::new().append(true).open(&events_path) {
            Ok(file) => file,
            Err(error) => {
                let _ = sender.send(LearnJobUpdate::IoError(format!(
                    "failed to open {} for append: {error}",
                    events_path.display()
                )));
                return;
            }
        };
        let mut writer = BufWriter::new(file);
        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    let _ = sender.send(LearnJobUpdate::IoError(format!(
                        "failed reading patch-learning stdout: {error}"
                    )));
                    return;
                }
            };
            if let Err(error) = writeln!(writer, "{line}").and_then(|_| writer.flush()) {
                let _ = sender.send(LearnJobUpdate::IoError(format!(
                    "failed persisting patch-learning event: {error}"
                )));
                return;
            }
            match LearnEvent::parse(&line) {
                Ok(event) => {
                    let _ = sender.send(LearnJobUpdate::Event(event));
                }
                Err(error) => {
                    let _ = sender.send(LearnJobUpdate::ProtocolError { line, error });
                }
            }
        }
    })
}

fn spawn_stderr_reader(
    stderr: ChildStderr,
    file: File,
    sender: Sender<LearnJobUpdate>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut writer = BufWriter::new(file);
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    if let Err(error) = writeln!(writer, "{line}").and_then(|_| writer.flush()) {
                        let _ = sender.send(LearnJobUpdate::IoError(format!(
                            "failed persisting patch-learning stderr: {error}"
                        )));
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(LearnJobUpdate::IoError(format!(
                        "failed reading patch-learning stderr: {error}"
                    )));
                    return;
                }
            }
        }
    })
}

fn exit_update(status: ExitStatus) -> LearnJobUpdate {
    LearnJobUpdate::Exited {
        success: status.success(),
        code: status.code(),
    }
}

fn create_job_dir(root: &Path) -> Result<(String, PathBuf), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create learn-jobs root {}: {error}", root.display()))?;
    for _ in 0..16 {
        let id = format!(
            "{}-{}-{}",
            unix_ms()?,
            std::process::id(),
            NEXT_JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let path = root.join(&id);
        match fs::create_dir(&path) {
            Ok(()) => return Ok((id, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("failed to create learn job {}: {error}", path.display()))
            }
        }
    }
    Err("failed to allocate a unique learn-job id".to_string())
}

fn validate_job_id(job_id: &str) -> Result<(), String> {
    if job_id.is_empty()
        || job_id == "."
        || job_id == ".."
        || job_id.contains('/')
        || job_id.contains('\\')
    {
        return Err("invalid learn-job id".to_string());
    }
    Ok(())
}

fn unix_ms() -> Result<u128, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|error| format!("system clock is before UNIX epoch: {error}"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode {}: {error}", path.display()))?;
    fs::write(path, bytes).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eseq-learn-job-{label}-{}-{}",
            std::process::id(),
            NEXT_JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn executable_script(root: &Path, body: &str) -> PathBuf {
        let path = root.join("fake-dgenlisp");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn spec(root: &Path, plan_only: bool) -> LearnJobSpec {
        let patch = root.join("source.lisp");
        let target = root.join("target.wav");
        fs::write(&patch, "(param gain @default 0.5 @min 0 @max 1)").unwrap();
        fs::write(&target, b"wav").unwrap();
        LearnJobSpec {
            patch_path: patch,
            patch_source: "(param gain @default 0.5 @min 0 @max 1)".to_string(),
            target_path: target,
            seed: LearnSeed {
                params: BTreeMap::from([("gain".to_string(), 0.5)]),
            },
            epochs: 50,
            pitch_hz: None,
            gate_frames: None,
            plan_only,
        }
    }

    #[test]
    fn event_parser_keeps_natural_params_and_optional_steps() {
        let event = LearnEvent::parse(
            r#"{"type":"epoch","epoch":2,"total":50,"loss":0.4,"params":{"cutoff":1234.5}}"#,
        )
        .unwrap();
        assert_eq!(
            event,
            LearnEvent::Epoch {
                epoch: 2,
                total: 50,
                loss: 0.4,
                params: BTreeMap::from([("cutoff".to_string(), 1234.5)]),
                steps: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn launcher_snapshots_stream_and_reloads_job() {
        let root = temp_dir("stream");
        let tool = executable_script(
            &root,
            "printf '%s\\n' '{\"type\":\"plan\",\"learnable\":[\"gain\"],\"frozen\":[],\"unsupported\":[],\"seed_echo\":{\"gain\":0.5},\"pitch_hz\":49.2,\"gate_frames\":8820,\"crop_frames\":32768}'\nprintf '%s\\n' '{\"type\":\"result\",\"improvement_pct\":54.2,\"abs_distance\":0.0116,\"basin_check\":\"ok\",\"deltas\":{\"gain\":{\"from\":0.5,\"to\":0.7}},\"final_wav\":\"final.wav\"}'",
        );
        let launcher = LearnJobLauncher::new(tool, root.join("jobs"));
        let job = launcher.launch(spec(&root, false)).unwrap();
        let id = job.job_id.clone();
        let mut updates = Vec::new();
        loop {
            let update = job.receiver.recv_timeout(Duration::from_secs(3)).unwrap();
            let exited = matches!(update, LearnJobUpdate::Exited { .. });
            updates.push(update);
            if exited {
                break;
            }
        }
        assert!(matches!(updates[0], LearnJobUpdate::Event(LearnEvent::Plan { .. })));
        assert!(updates.iter().any(|update| matches!(
            update,
            LearnJobUpdate::Event(LearnEvent::Result { improvement_pct, .. }) if *improvement_pct == 54.2
        )));
        assert!(matches!(updates.last(), Some(LearnJobUpdate::Exited { success: true, .. })));

        let snapshot = launcher.load(&id).unwrap();
        assert_eq!(snapshot.request.argv.first().map(String::as_str), Some("train"));
        assert_eq!(fs::read_to_string(snapshot.job_dir.join("patch.lisp")).unwrap(),
            "(param gain @default 0.5 @min 0 @max 1)");
        assert!(matches!(snapshot.terminal_event(), Some(LearnEvent::Result { .. })));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launcher_snapshots_relative_patch_assets() {
        let root = temp_dir("assets");
        fs::create_dir_all(root.join("waves")).unwrap();
        fs::write(root.join("waves/bank.json"), "[0.1, 0.2]").unwrap();
        let tool = executable_script(
            &root,
            "printf '%s\\n' '{\"type\":\"plan\",\"learnable\":[\"gain\"],\"frozen\":[],\"unsupported\":[],\"seed_echo\":{\"gain\":0.5},\"pitch_hz\":49.2,\"gate_frames\":8820,\"crop_frames\":32768}'",
        );
        let launcher = LearnJobLauncher::new(tool, root.join("jobs"));
        let mut job_spec = spec(&root, true);
        job_spec.patch_source =
            "(def bank (tensor @shape [2] @file \"waves/bank.json\"))\n(param gain @default 0.5 @min 0 @max 1)"
                .to_string();
        let job = launcher.launch(job_spec).unwrap();
        loop {
            if matches!(
                job.receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
                LearnJobUpdate::Exited { .. }
            ) {
                break;
            }
        }
        assert_eq!(
            fs::read_to_string(job.job_dir.join("waves/bank.json")).unwrap(),
            "[0.1, 0.2]"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_stdout_is_persisted_and_reported() {
        let root = temp_dir("protocol-error");
        let tool = executable_script(&root, "printf '%s\\n' 'not-json'");
        let launcher = LearnJobLauncher::new(tool, root.join("jobs"));
        let job = launcher.launch(spec(&root, true)).unwrap();
        let mut saw_error = false;
        loop {
            let update = job.receiver.recv_timeout(Duration::from_secs(3)).unwrap();
            saw_error |= matches!(update, LearnJobUpdate::ProtocolError { .. });
            if matches!(update, LearnJobUpdate::Exited { .. }) {
                break;
            }
        }
        assert!(saw_error);
        let snapshot = launcher.load(&job.job_id).unwrap();
        assert_eq!(snapshot.protocol_errors.len(), 1);
        assert_eq!(fs::read_to_string(snapshot.job_dir.join("events.jsonl")).unwrap(), "not-json\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_the_child_and_still_reports_exit() {
        let root = temp_dir("cancel");
        let tool = executable_script(
            &root,
            "trap 'exit 0' TERM\nprintf '%s\\n' '{\"type\":\"plan\",\"learnable\":[\"gain\"],\"frozen\":[],\"unsupported\":[],\"seed_echo\":{\"gain\":0.5},\"pitch_hz\":49.2,\"gate_frames\":8820,\"crop_frames\":32768}'\nwhile :; do sleep 0.1; done",
        );
        let launcher = LearnJobLauncher::new(tool, root.join("jobs"));
        let job = launcher.launch(spec(&root, false)).unwrap();
        assert!(matches!(
            job.receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
            LearnJobUpdate::Event(LearnEvent::Plan { .. })
        ));
        job.cancel().unwrap();
        loop {
            if matches!(
                job.receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
                LearnJobUpdate::Exited { .. }
            ) {
                break;
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}
