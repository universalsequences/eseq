mod accumulator;
#[allow(dead_code)]
mod agent;
mod audio;
mod audiograph;
#[allow(dead_code)]
mod delay;
#[allow(dead_code)]
mod effects;
#[allow(dead_code)]
mod filter;
#[allow(dead_code)]
mod gatepitch;
#[allow(dead_code)]
mod lisp_effect;
mod project;
mod recorder;
#[allow(dead_code)]
mod reverb;
mod sampler;
mod scale;
mod scheduler;
mod scheduled_event;
#[allow(dead_code)]
mod sequencer;
#[allow(dead_code)]
mod stereo_panner;
mod ui;
#[allow(dead_code)]
mod voice;
#[allow(dead_code)]
mod voice_modulator;

use std::ffi::CString;
use std::sync::Arc;
use std::time::Duration;

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.trim().parse::<i32>().ok()
}

fn recommended_worker_count() -> i32 {
    6
}

fn suspend_terminal(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> std::io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::PopKeyboardEnhancementFlags,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_terminal(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )?;
    terminal.clear()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Ensure samples/, effects/, and instruments/ directories exist
    std::fs::create_dir_all("samples").ok();
    std::fs::create_dir_all("effects").ok();
    std::fs::create_dir_all("instruments").ok();

    // Query audio device
    let (sample_rate, channels) = audio::query_device_config()?;
    let block_size: usize = 512;

    // Initialize audiograph engine
    unsafe {
        audiograph::initialize_engine(block_size as i32, sample_rate as i32);
    }

    let label = CString::new("sequencer").unwrap();
    let lg = unsafe {
        audiograph::create_live_graph(64, block_size as i32, label.as_ptr(), channels as i32)
    };
    if lg.is_null() {
        return Err("Failed to create live graph".into());
    }

    // Create two bus nodes (L and R)
    let bus_l_name = CString::new("bus_L").unwrap();
    let bus_r_name = CString::new("bus_R").unwrap();
    let bus_l_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_l_name.as_ptr()) };
    let bus_r_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_r_name.as_ptr()) };

    // bus_L → DAC channel 0, bus_R → DAC channel 1
    unsafe {
        audiograph::graph_connect(lg, bus_l_id, 0, 0, 0);
        if channels > 1 {
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 1);
        } else {
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 0);
        }
    }

    // Create global reverb bus and reverb node
    let reverb_bus_name = CString::new("reverb_bus").unwrap();
    let reverb_bus_id = unsafe { audiograph::live_add_gain(lg, 1.0, reverb_bus_name.as_ptr()) };

    let reverb_node_name = CString::new("reverb").unwrap();
    let reverb_node_id = unsafe {
        audiograph::add_node(
            lg,
            reverb::reverb_vtable(),
            reverb::REVERB_STATE_SIZE * std::mem::size_of::<f32>(),
            reverb_node_name.as_ptr(),
            1,
            2, // 1 mono input, 2 stereo outputs
            std::ptr::null(),
            0,
        )
    };

    // Wire: reverb_bus → reverb_node → bus_L / bus_R
    unsafe {
        audiograph::graph_connect(lg, reverb_bus_id, 0, reverb_node_id, 0);
        audiograph::graph_connect(lg, reverb_node_id, 0, bus_l_id, 0);
        audiograph::graph_connect(lg, reverb_node_id, 1, bus_r_id, 0);
    }

    let workers = env_i32("TINYSEQ_AUDIOGRAPH_WORKERS")
        .unwrap_or_else(recommended_worker_count)
        .max(0);
    let mach_rt_default = cfg!(target_os = "macos") && workers > 0;
    let mach_rt = env_flag("TINYSEQ_AUDIOGRAPH_MACH_RT", mach_rt_default);
    let rt_log = env_flag("TINYSEQ_AUDIOGRAPH_RT_LOG", false);

    unsafe {
        audiograph::enable_rt_logging(rt_log);
        audiograph::enable_rt_time_constraint(mach_rt);
        audiograph::engine_start_workers(workers);
    }
    eprintln!(
        "audiograph: started {workers} worker(s), Mach RT {}, OS workgroup exposed via FFI only",
        if mach_rt { "enabled" } else { "disabled" }
    );

    // Create shared sequencer state (start with 0 tracks)
    let state = Arc::new(sequencer::SequencerState::new(0, vec![]));
    let master_recorder = Arc::new(recorder::MasterRecorder::new(sample_rate, channels));

    // Create psc channel for keyboard triggers
    let (keyboard_tx, keyboard_rx) = std::sync::mpsc::channel();

    // Build cpal audio stream
    let stream = audio::build_output_stream(
        lg,
        Arc::clone(&state),
        sample_rate,
        channels as usize,
        block_size,
        Arc::clone(&master_recorder),
        keyboard_rx,
    )?;

    let args: Vec<String> = std::env::args().collect();
    let lg_ptr = audiograph::LiveGraphPtr(lg);
    let mut app = ui::App::new(
        Arc::clone(&state),
        lg_ptr,
        sample_rate,
        ui::AudioBuses {
            bus_l_id,
            bus_r_id,
            reverb_bus_id,
            reverb_node_id,
        },
        master_recorder,
        keyboard_tx,
    );
    if args.iter().any(|arg| arg == "--headless-custom-repro") {
        run_headless_custom_repro(&mut app)?;
        drop(stream);
        unsafe {
            audiograph::clear_os_workgroup();
            audiograph::engine_stop_workers();
            audiograph::destroy_live_graph(lg);
        }
        return Ok(());
    }

    // Setup terminal
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Main loop
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        app.handle_input()?;
        if app.ui.should_quit {
            break;
        }
        if app.has_pending_editor() {
            suspend_terminal(&mut terminal)?;
            app.run_pending_editor();
            resume_terminal(&mut terminal)?;
        }
    }

    // Cleanup
    suspend_terminal(&mut terminal)?;
    drop(stream);

    unsafe {
        audiograph::clear_os_workgroup();
        audiograph::engine_stop_workers();
        audiograph::destroy_live_graph(lg);
    }

    Ok(())
}

fn run_headless_custom_repro(app: &mut ui::App) -> Result<(), Box<dyn std::error::Error>> {
    let instrument_names = lisp_effect::list_saved_instruments();
    if instrument_names.is_empty() {
        return Err("No saved instruments found in instruments/".into());
    }

    let selected: Vec<String> = instrument_names.into_iter().take(5).collect();
    if selected.len() < 5 {
        return Err(format!(
            "Need at least 5 saved instruments for headless repro, found {}",
            selected.len()
        )
        .into());
    }

    println!(
        "headless custom repro: adding {} instruments",
        selected.len()
    );
    for (idx, name) in selected.iter().enumerate() {
        println!("step {}: adding instrument '{}'", idx + 1, name);
        let track_idx = app.add_saved_instrument_track_sync(name)?;
        println!("step {}: added as track {}", idx + 1, track_idx);
        if idx + 1 < selected.len() {
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    println!("headless custom repro complete; exiting after 5 seconds");
    std::thread::sleep(Duration::from_secs(5));
    Ok(())
}
