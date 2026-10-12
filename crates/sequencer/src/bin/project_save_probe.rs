//! Times saving a real project headlessly: load it, then run the save path
//! (capture + serialize + write) a few times and report each phase.
//!
//! `cargo run --release --bin project_save_probe -- <project-name>`
//!
//! `SAVE_PROBE_RUNS=n` sets the number of saves (default 3) and
//! `SAVE_PROBE_KEEP=1` keeps the last one as `zz-save-probe-<pid>`;
//! `-- <project-name> --dump-to <path>` writes a project back inline instead.

use std::path::Path;
use std::time::Instant;

use sequencer::{app, engine, project};

#[path = "probe_support/mod.rs"]
mod probe_support;
use probe_support::{pump_graph, GraphGuard};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args()
        .nth(1)
        .ok_or("usage: project_save_probe <project-name>")?;
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(manifest_dir)?;

    // `--dump-to <path>`: write the project back inline, as the loader sees
    // it, and stop. Diffing dumps of saves from two builds checks that a
    // format change is lossless.
    let mut args = std::env::args().skip(2);
    if args.next().as_deref() == Some("--dump-to") {
        let dump = args.next().ok_or("--dump-to needs a path")?;
        std::fs::write(dump, serde_json::to_vec(&project::load_project(&name)?)?)?;
        return Ok(());
    }
    let started = Instant::now();
    let file = project::load_project(&name)?;
    eprintln!("[save-probe] parse {:?}", started.elapsed());
    let started = Instant::now();
    let json = serde_json::to_string(&file)?;
    eprintln!(
        "[save-probe] serialize-only {:?} ({} bytes)",
        started.elapsed(),
        json.len()
    );
    drop((file, json));

    let eng = engine::init_headless_engine(44_100, 2)?;
    let lg_ptr = eng.lg_ptr;
    let _guard = GraphGuard { lg: lg_ptr.0 };
    let mut app = app::App::new(
        eng.state.clone(),
        lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder.clone(),
        eng.keyboard_tx.clone(),
    );
    let started = Instant::now();
    app.queue_project_load_named(&name)?;
    for _ in 0..4096 {
        if !app.has_pending_project_load() {
            break;
        }
        app.advance_pending_project_load()?;
        pump_graph(lg_ptr, 2);
    }
    if app.has_pending_project_load() {
        return Err(format!("project '{name}' did not finish loading").into());
    }
    eprintln!("[save-probe] load {:?}", started.elapsed());

    let probe_name = format!("zz-save-probe-{}", std::process::id());
    let runs: usize = std::env::var("SAVE_PROBE_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    for run in 0..runs {
        let started = Instant::now();
        app.begin_project_save(Some(&probe_name))?;
        let blocked = started.elapsed();
        for outcome in app.wait_for_project_saves() {
            outcome.result?;
        }
        eprintln!(
            "[save-probe] run {run} main thread {blocked:?}, written after {:?}",
            started.elapsed()
        );
    }
    if std::env::var_os("SAVE_PROBE_KEEP").is_some() {
        return Ok(());
    }
    let _ = std::fs::remove_file(
        sequencer::app_paths::app_paths()
            .projects_dir()
            .join(format!("{probe_name}.json")),
    );
    Ok(())
}
