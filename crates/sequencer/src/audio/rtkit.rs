//! Linux RealtimeKit fallback for audiograph threads.
//!
//! Direct `SCHED_FIFO` promotion remains in the C engine. Threads denied with
//! `EPERM` hand their Linux TID to this module; one ordinary-priority helper
//! owns the system-bus connection and performs every blocking D-Bus call.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use zbus::blocking::{connection, Connection, Proxy};

const RTKIT_DESTINATION: &str = "org.freedesktop.RealtimeKit1";
const RTKIT_PATH: &str = "/org/freedesktop/RealtimeKit1";
const RTKIT_INTERFACE: &str = "org.freedesktop.RealtimeKit1";
const METHOD_TIMEOUT: Duration = Duration::from_secs(2);
const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(5);

static COORDINATOR: OnceLock<Arc<Coordinator>> = OnceLock::new();
static UNAVAILABLE_WARNING_LOGGED: AtomicBool = AtomicBool::new(false);

type WorkerResult = Result<i32, String>;

struct WorkerRequest {
    tid: u64,
    requested_priority: i32,
    result: mpsc::SyncSender<WorkerResult>,
}

struct Coordinator {
    workers: mpsc::Sender<WorkerRequest>,
    callback_tid: AtomicU64,
    callback_priority: AtomicI32,
    rt_log: bool,
}

struct RtKitClient {
    connection: Connection,
    max_priority: i32,
}

/// Start the non-realtime owner of all RealtimeKit IPC. This is called before
/// any graph worker or cpal callback can request promotion.
pub(super) fn start(rt_log: bool) {
    if COORDINATOR.get().is_some() {
        return;
    }

    let (workers, requests) = mpsc::channel();
    let coordinator = Arc::new(Coordinator {
        workers,
        callback_tid: AtomicU64::new(0),
        callback_priority: AtomicI32::new(0),
        rt_log,
    });
    let helper_coordinator = Arc::clone(&coordinator);
    if let Err(error) = std::thread::Builder::new()
        .name("eseq-rtkit".to_string())
        .spawn(move || helper_main(helper_coordinator, requests))
    {
        warn_unavailable(&format!("cannot start helper thread: {error}"));
        return;
    }
    if COORDINATOR.set(coordinator).is_err() {
        return;
    }

    unsafe {
        super::audiograph::engine_set_rtkit_hooks(
            Some(eseq_rtkit_make_worker_realtime),
            Some(eseq_rtkit_request_callback_realtime),
        );
    }
}

fn helper_main(coordinator: Arc<Coordinator>, requests: mpsc::Receiver<WorkerRequest>) {
    let mut client: Option<Result<RtKitClient, String>> = None;
    loop {
        match requests.recv_timeout(CALLBACK_POLL_INTERVAL) {
            Ok(request) => {
                let result = promote(&mut client, request.tid, request.requested_priority, false);
                report_result(&coordinator, request.tid, false, &result);
                let _ = request.result.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let callback_tid = coordinator.callback_tid.swap(0, Ordering::AcqRel);
        if callback_tid != 0 {
            let requested = coordinator.callback_priority.load(Ordering::Acquire);
            let result = promote(&mut client, callback_tid, requested, true);
            report_result(&coordinator, callback_tid, true, &result);
            unsafe {
                super::audiograph::engine_record_rtkit_callback_result(callback_tid as libc::pid_t);
                let achieved = super::audiograph::format_rt_status(super::audiograph::rt_status());
                eprintln!("audiograph: realtime scheduling achieved: {achieved}");
            }
        }
    }
}

fn promote(
    client: &mut Option<Result<RtKitClient, String>>,
    tid: u64,
    requested_priority: i32,
    callback: bool,
) -> WorkerResult {
    if client.is_none() {
        *client = Some(RtKitClient::connect());
    }
    // The bool marks errors worth caching: a connection, property, or
    // transport failure will not improve for the next thread a few
    // microseconds later, so caching it means a missing or wedged service
    // adds at most one bounded timeout to startup rather than one per
    // worker. A method error REPLY is different — the service is alive and
    // refused this one request (e.g. an rtkit per-user thread quota), so
    // later threads, above all the audio callback which asks last, must
    // still get their own attempt.
    let result: Result<i32, (bool, String)> = (|| {
        let connected = client.as_ref().expect("rtkit state initialized");
        let connected = connected.as_ref().map_err(|error| (true, error.clone()))?;
        let priority = clamp_priority(requested_priority, connected.max_priority, callback)
            .map_err(|error| (true, error))?;
        let proxy = connected.proxy().map_err(|error| (true, error))?;
        proxy
            .call::<_, _, ()>("MakeThreadRealtime", &(tid, priority as u32))
            .map_err(|error| {
                let transport_failure = !matches!(error, zbus::Error::MethodError(..));
                (transport_failure, format!("MakeThreadRealtime failed: {error}"))
            })?;
        Ok(priority)
    })();
    match result {
        Ok(priority) => Ok(priority),
        Err((cache_failure, error)) => {
            if cache_failure {
                *client = Some(Err(error.clone()));
            }
            Err(error)
        }
    }
}

fn report_result(coordinator: &Coordinator, tid: u64, callback: bool, result: &WorkerResult) {
    match result {
        Ok(priority) if coordinator.rt_log => {
            let role = if callback {
                "audio callback thread"
            } else {
                "worker"
            };
            eprintln!(
                "[audiograph] {role} tid {tid} set SCHED_RR priority {priority} via RealtimeKit"
            );
        }
        Err(error) => warn_unavailable(error),
        _ => {}
    }
}

fn warn_unavailable(error: &str) {
    if !UNAVAILABLE_WARNING_LOGGED.swap(true, Ordering::AcqRel) {
        eprintln!(
            "[audiograph] WARN: direct SCHED_FIFO promotion was denied and the RealtimeKit fallback was unavailable ({error}); audio continues at normal priority. For full FIFO priority, grant RLIMIT_RTPRIO with limits.conf/systemd LimitRTPRIO or CAP_SYS_NICE"
        );
    }
}

impl RtKitClient {
    fn connect() -> Result<Self, String> {
        let connection = connection::Builder::system()
            .map_err(|error| format!("cannot open the system bus: {error}"))?
            .method_timeout(METHOD_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot open the system bus: {error}"))?;
        let proxy = Proxy::new(&connection, RTKIT_DESTINATION, RTKIT_PATH, RTKIT_INTERFACE)
            .map_err(|error| format!("cannot create RealtimeKit proxy: {error}"))?;
        let max_priority: i32 = proxy
            .get_property("MaxRealtimePriority")
            .map_err(|error| format!("cannot read MaxRealtimePriority: {error}"))?;
        let rttime_usec_max: i64 = proxy
            .get_property("RTTimeUSecMax")
            .map_err(|error| format!("cannot read RTTimeUSecMax: {error}"))?;
        set_rttime_limit(rttime_usec_max)?;
        Ok(Self {
            connection,
            max_priority,
        })
    }

    fn proxy(&self) -> Result<Proxy<'_>, String> {
        Proxy::new(
            &self.connection,
            RTKIT_DESTINATION,
            RTKIT_PATH,
            RTKIT_INTERFACE,
        )
        .map_err(|error| format!("cannot create RealtimeKit proxy: {error}"))
    }
}

fn clamp_priority(requested: i32, cap: i32, callback: bool) -> Result<i32, String> {
    if cap < 1 {
        return Err(format!("RealtimeKit reported invalid priority cap {cap}"));
    }
    let worker_cap = if cap > 1 { cap - 1 } else { cap };
    let role_cap = if callback { cap } else { worker_cap };
    Ok(requested.clamp(1, role_cap))
}

fn set_rttime_limit(rttime_usec_max: i64) -> Result<(), String> {
    if rttime_usec_max <= 0 {
        return Err(format!(
            "RealtimeKit reported invalid RTTimeUSecMax {rttime_usec_max}"
        ));
    }

    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut current) } != 0 {
        return Err(format!(
            "getrlimit(RLIMIT_RTTIME) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let rtkit_limit = rttime_usec_max as libc::rlim_t;
    let limit = if current.rlim_max == libc::RLIM_INFINITY {
        rtkit_limit
    } else {
        current.rlim_max.min(rtkit_limit)
    };
    let requested = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &requested) } != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_RTTIME={limit}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Worker entry points may wait here because they have not begun graph work.
/// Only the helper thread performs D-Bus IPC.
#[no_mangle]
pub extern "C" fn eseq_rtkit_make_worker_realtime(
    tid: libc::pid_t,
    requested_priority: libc::c_int,
) -> libc::c_int {
    let Some(coordinator) = COORDINATOR.get() else {
        return 0;
    };
    let (result, response) = mpsc::sync_channel(1);
    if coordinator
        .workers
        .send(WorkerRequest {
            tid: tid as u64,
            requested_priority,
            result,
        })
        .is_err()
    {
        return 0;
    }
    match response.recv() {
        Ok(Ok(_)) => 1,
        _ => 0,
    }
}

/// The audio callback publishes two atomics and returns. The helper notices
/// the request on its next bounded poll; no lock, allocation, or IPC occurs on
/// the realtime thread.
#[no_mangle]
pub extern "C" fn eseq_rtkit_request_callback_realtime(
    tid: libc::pid_t,
    requested_priority: libc::c_int,
) -> libc::c_int {
    let Some(coordinator) = COORDINATOR.get() else {
        return 0;
    };
    coordinator
        .callback_priority
        .store(requested_priority, Ordering::Relaxed);
    coordinator
        .callback_tid
        .store(tid as u64, Ordering::Release);
    1
}

#[cfg(test)]
mod tests {
    use super::clamp_priority;

    #[test]
    fn priority_clamping_keeps_callback_at_or_above_workers() {
        assert_eq!(clamp_priority(20, 15, false).unwrap(), 14);
        assert_eq!(clamp_priority(21, 15, true).unwrap(), 15);
        assert_eq!(clamp_priority(20, 1, false).unwrap(), 1);
        assert_eq!(clamp_priority(21, 1, true).unwrap(), 1);
        assert!(clamp_priority(20, 0, false).is_err());
    }
}
