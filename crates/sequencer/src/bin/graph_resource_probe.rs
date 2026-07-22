use std::path::Path;

use sequencer::{app, audiograph, engine};

const BLOCK_SIZE: usize = 512;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "endingofrio".to_string());
    let effect = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Limiter".to_string());

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(manifest_dir)?;

    eprintln!("[graph-resource-probe] project={project} effect={effect}");
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

    app.queue_project_load_named(&project)?;
    for tick in 0..1024 {
        if !app.has_pending_project_load() {
            break;
        }
        app.advance_pending_project_load()?;
        pump_graph(eng.lg_ptr, 8);
        if tick % 16 == 0 {
            eprintln!("[graph-resource-probe] load tick={tick}");
        }
    }
    if app.has_pending_project_load() {
        return Err(format!("project '{project}' did not finish loading").into());
    }
    pump_graph(lg_ptr, 32);
    dump_graph_if_requested(lg_ptr, "after-load");
    eprintln!(
        "[graph-resource-probe] loaded tracks={} buses={}",
        app.tracks.len(),
        app.buses.len()
    );

    eprintln!("[graph-resource-probe] adding track 0 built-in effect '{effect}'");
    match app.add_builtin_effect_sync(0, &effect) {
        Ok(slot) => eprintln!("[graph-resource-probe] queued effect slot={slot}"),
        Err(error) => eprintln!("[graph-resource-probe] add effect returned error: {error}"),
    }
    pump_graph(lg_ptr, 64);
    dump_graph_if_requested(lg_ptr, "after-add-effect");
    eprintln!("[graph-resource-probe] done; inspect TINYSEQ_AUDIOGRAPH_TRACE edits_ok lines");

    Ok(())
}

fn dump_graph_if_requested(lg: sequencer::audiograph::LiveGraphPtr, label: &str) {
    if std::env::var_os("ESEQ_GRAPH_RESOURCE_DUMP").is_none() {
        return;
    }
    eprintln!("[graph-resource-probe] graph dump {label} begin");
    unsafe {
        audiograph::debug_dump_graph(lg.0);
    }
    eprintln!("[graph-resource-probe] graph dump {label} end");
}

fn pump_graph(lg: sequencer::audiograph::LiveGraphPtr, blocks: usize) {
    let mut output = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..blocks {
        unsafe {
            lg.process_next_block(output.as_mut_ptr(), BLOCK_SIZE as i32);
        }
    }
}

struct GraphGuard {
    lg: *mut audiograph::LiveGraph,
}

impl Drop for GraphGuard {
    fn drop(&mut self) {
        unsafe {
            audiograph::clear_os_workgroup();
            audiograph::engine_stop_workers();
            audiograph::destroy_live_graph(self.lg);
        }
    }
}
