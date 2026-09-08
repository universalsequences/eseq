use crate::bounce::{
    worker::{export_project, ExportOptions},
    BounceCancellation,
};
use std::path::PathBuf;

static CANCEL: BounceCancellation = BounceCancellation::new();
extern "C" fn cancel_export(_: libc::c_int) {
    CANCEL.cancel();
}

/// Entry point for a dedicated worker process, before any live engine exists.
pub fn run(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = None;
    let mut destination = None;
    let mut sample_rate = 48_000;
    let mut tail_seconds = 10.0;
    let mut replace = false;
    let mut start = None;
    let mut end = None;
    let mut cancel_path = None;
    let mut status_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" => {
                project = Some(PathBuf::from(args.next().ok_or("--project needs a path")?))
            }
            "--out" => destination = Some(PathBuf::from(args.next().ok_or("--out needs a path")?)),
            "--sample-rate" => {
                sample_rate = args.next().ok_or("--sample-rate needs a number")?.parse()?
            }
            "--tail" => tail_seconds = args.next().ok_or("--tail needs seconds")?.parse()?,
            "--replace" => replace = true,
            "--start-beat" => {
                start = Some(
                    args.next()
                        .ok_or("--start-beat needs a number")?
                        .parse::<f64>()?,
                )
            }
            "--end-beat" => {
                end = Some(
                    args.next()
                        .ok_or("--end-beat needs a number")?
                        .parse::<f64>()?,
                )
            }
            "--status-file" => {
                status_path = Some(std::path::absolute(
                    args.next().ok_or("--status-file needs a path")?,
                )?)
            }
            "--cancel-file" => {
                cancel_path = Some(std::path::absolute(
                    args.next().ok_or("--cancel-file needs a path")?,
                )?)
            }
            "--help" | "-h" => {
                println!("eseq_export --project PATH --out PATH [--sample-rate 48000] [--tail 10] [--start-beat N --end-beat N] [--replace] [--cancel-file PATH]");
                return Ok(());
            }
            _ => return Err(format!("Unknown option: {arg}").into()),
        }
    }
    let selection = match (start, end) {
        (None, None) => None,
        (Some(start), Some(end)) => Some((start, end)),
        _ => return Err("--start-beat and --end-beat must be supplied together".into()),
    };
    let options = ExportOptions {
        project: std::path::absolute(project.ok_or("--project is required")?)?,
        destination: std::path::absolute(destination.ok_or("--out is required")?)?,
        sample_rate,
        tail_seconds,
        selection,
        replace,
        cancel_path,
    };
    crate::app_paths::init()?;
    // The signal handler only stores a lock-free atomic flag. Cooperative
    // cancellation unwinds the worker and removes its unpublished WAV.
    unsafe {
        if libc::signal(
            libc::SIGINT,
            cancel_export as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let worker_status_path = status_path.clone();
    let result = std::thread::Builder::new()
        .name("song-export".into())
        .stack_size(crate::REQUIRED_THREAD_STACK_SIZE)
        .spawn(move || {
            let mut last_percent = None;
            export_project(&options, &CANCEL, |progress| {
                let percent =
                    progress.rendered_frames.saturating_mul(100) / progress.total_render_frames;
                if last_percent != Some(percent) {
                    eprintln!("Export {percent}%");
                    if let Some(path) = &worker_status_path {
                        if super::job::write_status(
                            path,
                            &super::job::WorkerStatus::Rendering { percent },
                        )
                        .is_err()
                        {
                            CANCEL.cancel();
                        }
                    }
                    last_percent = Some(percent);
                }
            })
        })?
        .join()
        .map_err(|_| std::io::Error::other("Export worker panicked"))?;
    if let Some(path) = &status_path {
        let status = match &result {
            Ok(summary) => super::job::WorkerStatus::Completed {
                frames: summary.frames,
                tail_warning: summary.tail_may_be_truncated(),
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                super::job::WorkerStatus::Cancelled
            }
            Err(error) => super::job::WorkerStatus::Failed {
                message: error.to_string(),
            },
        };
        super::job::write_status(path, &status)?;
    }
    let result = result?;
    println!(
        "Exported {} stereo frames at {} Hz (peak {:.3})",
        result.frames, result.sample_rate, result.peak
    );
    if result.tail_may_be_truncated() {
        eprintln!("Audio remains at the end of the tail; consider a longer --tail.");
    }
    Ok(())
}
