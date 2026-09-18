//! A local, same-user diagnostic endpoint. Requests contain only measurement
//! duration/label; file output belongs to the CLI, never to the application.
use super::*;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) struct Server {
    pub receiver: mpsc::Receiver<PendingRequest>,
    path: PathBuf,
    stop: Arc<AtomicBool>,
    listener: Option<std::thread::JoinHandle<()>>,
}

fn socket_directory() -> PathBuf {
    std::env::temp_dir().join(format!("eseq-ui-{}", unsafe { libc::geteuid() }))
}

fn socket_path(pid: u32) -> PathBuf {
    socket_directory().join(format!("{pid}.sock"))
}

fn respond(stream: &mut UnixStream, response: serde_json::Value) {
    let _ = serde_json::to_writer(&mut *stream, &response);
    let _ = stream.write_all(b"\n");
}

impl Server {
    pub fn start(waker: Option<eseqlisp::backend::EventLoopWaker>) -> io::Result<Self> {
        let dir = socket_directory();
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(&dir)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "UI benchmark socket directory must be private and owned by this user",
            ));
        }
        let path = socket_path(std::process::id());
        // A previous process may have left a socket behind at this PID. Do not
        // unlink a live endpoint (including a second server in this process).
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            use std::os::unix::fs::FileTypeExt;
            if !metadata.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "benchmark endpoint is not a socket",
                ));
            }
            match UnixStream::connect(&path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "benchmark endpoint is already listening",
                    ))
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(&path)?;
                }
                Err(error) => return Err(error),
            }
        }
        let socket = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let listener = std::thread::Builder::new()
            .name("ui-benchmark-control".into())
            .spawn(move || {
                let busy = Arc::new(AtomicBool::new(false));
                for connection in socket.incoming() {
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(mut stream) = connection else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut bytes = Vec::new();
                    let parsed = io::BufReader::new((&mut stream).take(4097))
                        .read_until(b'\n', &mut bytes)
                        .map_err(|error| error.to_string())
                        .and_then(|_| {
                            if bytes.len() > 4096 {
                                return Err("request too large".into());
                            }
                            serde_json::from_slice::<Request>(&bytes)
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|request| {
                            request.validate()?;
                            Ok(request)
                        });
                    let request = match parsed {
                        Ok(request) => request,
                        Err(error) => {
                            respond(&mut stream, serde_json::json!({"error": error}));
                            continue;
                        }
                    };
                    if busy.swap(true, Ordering::AcqRel) {
                        respond(
                            &mut stream,
                            serde_json::json!({"error": "a UI benchmark is already running"}),
                        );
                        continue;
                    }
                    let (reply, response) = mpsc::channel();
                    let worker_busy = Arc::clone(&busy);
                    let worker_stop = Arc::clone(&thread_stop);
                    // One response worker at a time. It only transports the report;
                    // both CPU-clock reads still happen on the UI thread.
                    let worker = std::thread::Builder::new()
                        .name("ui-benchmark-response".into())
                        .spawn(move || {
                            loop {
                                match response.recv_timeout(Duration::from_millis(100)) {
                                    Ok(Ok(report)) => {
                                        respond(&mut stream, serde_json::json!({"report": report}));
                                        break;
                                    }
                                    Ok(Err(error)) => {
                                        respond(&mut stream, serde_json::json!({"error": error}));
                                        break;
                                    }
                                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                    Err(mpsc::RecvTimeoutError::Timeout)
                                        if worker_stop.load(Ordering::Relaxed) =>
                                    {
                                        break
                                    }
                                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                                }
                            }
                            worker_busy.store(false, Ordering::Release);
                        });
                    if worker.is_err() {
                        busy.store(false, Ordering::Release);
                        continue;
                    }
                    if let Err(error) = sender.try_send(PendingRequest { request, reply }) {
                        let pending = match error {
                            mpsc::TrySendError::Full(pending)
                            | mpsc::TrySendError::Disconnected(pending) => pending,
                        };
                        let _ = pending
                            .reply
                            .send(Err("UI benchmark request queue unavailable".into()));
                    } else if let Some(waker) = &waker {
                        waker.wake();
                    }
                }
            })?;
        Ok(Self {
            receiver,
            path,
            stop,
            listener: Some(listener),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.path); // Wake accept so shutdown does not leave a listener.
        if let Some(thread) = self.listener.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(2);
    let mut pid = None;
    let mut out = None;
    let mut request = Request {
        seconds: 20,
        warmup_seconds: 3,
        label: String::new(),
        cpu_only: false,
    };
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("metal_seq benchmark-ui --pid PID [--seconds 20] [--warmup 3] [--label NAME] [--out PATH] [--cpu-only]");
            return Ok(());
        }
        if arg == "--cpu-only" {
            request.cpu_only = true;
            continue;
        }
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--pid" => pid = Some(value.parse::<u32>()?),
            "--seconds" => request.seconds = value.parse()?,
            "--warmup" => request.warmup_seconds = value.parse()?,
            "--label" => request.label = value,
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown benchmark argument {arg}").into()),
        }
    }
    request.validate()?;
    let pid = pid.ok_or("--pid must name the running metal_seq process")?;
    let mut socket = UnixStream::connect(socket_path(pid))
        .map_err(|error| format!("cannot reach UI benchmark endpoint for PID {pid}: {error}; restart the app with a benchmark-enabled build"))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.set_read_timeout(Some(Duration::from_secs(
        request.seconds + request.warmup_seconds + 10,
    )))?;
    serde_json::to_writer(&mut socket, &request)?;
    socket.write_all(b"\n")?;
    let mut response = String::new();
    io::BufReader::new(socket.take(4 * 1024 * 1024)).read_line(&mut response)?;
    let response: serde_json::Value = serde_json::from_str(&response)?;
    if let Some(error) = response.get("error") {
        return Err(format!("UI benchmark: {error}").into());
    }
    let report = response.get("report").ok_or("missing benchmark report")?;
    let score = report["main_thread_cpu_ms_per_second"]
        .as_f64()
        .ok_or("missing UI CPU score")?;
    println!(
        "UI main thread: {score:.1} CPU ms/s ({:.1}% of one core)",
        score / 10.0
    );
    println!(
        "Presented FPS: {} | dispatch-to-present p50/p95/p99 ms: {} | workload changed: {}",
        report["presented_fps"], report["input_dispatch_to_present_ms"], report["workload_changed"]
    );
    println!(
        "Present intervals p50/p95/p99 ms: {} | polls: {} | syncs: {} | submitted frames: {}",
        report["presentation_interval_ms"],
        report["counters"]["poll_calls"],
        report["counters"]["syncs"],
        report["counters"]["submissions"]
    );
    for field in [
        "missing_presentation_feedback",
        "feedback_channel_overflows",
        "sample_overflows",
        "skipped_presentations",
    ] {
        if report[field].as_u64().is_some_and(|count| count > 0) {
            eprintln!("Measurement note: {field} = {}; inspect display guardrails before comparing CPU scores", report[field]);
        }
    }
    if report["workload_changed"] == true
        || report["diagnostic_flags"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty())
    {
        eprintln!("Measurement note: workload changed or other diagnostics were enabled; inspect the JSON report before comparison");
    }
    if let Some(out) = out {
        std::fs::write(&out, serde_json::to_vec_pretty(report)?)?;
        println!("Report: {}", out.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(value: serde_json::Value) -> serde_json::Value {
        let mut stream = UnixStream::connect(socket_path(std::process::id())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        serde_json::to_writer(&mut stream, &value).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut response = String::new();
        io::BufReader::new(stream).read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn endpoint_validates_requests_rejects_overlap_and_cleans_up() {
        let server = Server::start(None).unwrap();
        assert_eq!(
            std::fs::metadata(&server.path).unwrap().mode() & 0o777,
            0o600
        );
        assert!(
            matches!(Server::start(None), Err(error) if error.kind() == io::ErrorKind::AddrInUse)
        );
        let request = Request {
            seconds: 2,
            warmup_seconds: 0,
            label: "endpoint test".into(),
            cpu_only: false,
        };
        let mut invalid = serde_json::to_value(&request).unwrap();
        invalid["seconds"] = 0.into();
        assert!(exchange(invalid)["error"]
            .as_str()
            .unwrap()
            .contains("seconds"));
        assert!(matches!(
            server.receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let value = serde_json::to_value(&request).unwrap();
        let client_value = value.clone();
        let client = std::thread::spawn(move || exchange(client_value));
        let pending = server
            .receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(pending.request.label, "endpoint test");
        assert!(exchange(value)["error"]
            .as_str()
            .unwrap()
            .contains("already running"));
        pending
            .reply
            .send(Err("expected test response".into()))
            .unwrap();
        assert_eq!(client.join().unwrap()["error"], "expected test response");
        let path = server.path.clone();
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn endpoint_shutdown_releases_a_waiting_client() {
        let server = Server::start(None).unwrap();
        let client = std::thread::spawn(|| {
            let mut stream = UnixStream::connect(socket_path(std::process::id())).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let request = Request {
                seconds: 120,
                warmup_seconds: 0,
                label: String::new(),
                cpu_only: true,
            };
            serde_json::to_writer(&mut stream, &request).unwrap();
            stream.write_all(b"\n").unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let _pending = server
            .receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        drop(server);
        assert!(client.join().unwrap().is_empty());
    }
}
