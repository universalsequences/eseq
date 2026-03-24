use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sequencer::engine;
use sequencer::ui;

static CRASH_LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static CRASH_LOG_PATH: OnceLock<String> = OnceLock::new();

#[cfg(unix)]
mod crash_handler {
    use super::{CRASH_LOG, CRASH_LOG_PATH, OnceLock};
    use std::ffi::c_void;
    use std::io::Write;
    use std::mem::MaybeUninit;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static SAVED_TERMIOS: OnceLock<libc::termios> = OnceLock::new();
    static HAVE_TERMIOS: AtomicBool = AtomicBool::new(false);
    static CRASH_LOG_FD: AtomicI32 = AtomicI32::new(libc::STDERR_FILENO);

    unsafe extern "C" {
        fn backtrace(buffer: *mut *mut c_void, size: libc::c_int) -> libc::c_int;
        fn backtrace_symbols_fd(buffer: *const *mut c_void, size: libc::c_int, fd: libc::c_int);
    }

    pub fn install(log_fd: i32) {
        CRASH_LOG_FD.store(log_fd, Ordering::Relaxed);
        save_terminal_state();
        install_panic_hook();
        install_signal_handlers();
    }

    fn install_panic_hook() {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            restore_terminal();

            let mut bt = backtrace::Backtrace::new();
            bt.resolve();
            let message = format!(
                "\nsequencer panic: {panic_info}\ncrash log: {}\nbacktrace:\n{bt:?}\n",
                crash_log_path()
            );
            write_report(message.as_bytes());
            default_hook(panic_info);
        }));
    }

    fn install_signal_handlers() {
        for signal in [
            libc::SIGSEGV,
            libc::SIGABRT,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
        ] {
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_flags = libc::SA_RESETHAND | libc::SA_NODEFER;
                sa.sa_sigaction = signal_handler as *const () as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(signal, &sa, ptr::null_mut());
            }
        }
    }

    fn crash_log_path() -> &'static str {
        CRASH_LOG_PATH
            .get()
            .map(String::as_str)
            .unwrap_or("sequencer-crash.log")
    }

    fn save_terminal_state() {
        let mut termios = MaybeUninit::<libc::termios>::uninit();
        let rc = unsafe { libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) };
        if rc == 0 {
            let _ = SAVED_TERMIOS.set(unsafe { termios.assume_init() });
            HAVE_TERMIOS.store(true, Ordering::Relaxed);
        }
    }

    fn restore_terminal() {
        if HAVE_TERMIOS.load(Ordering::Relaxed) {
            if let Some(termios) = SAVED_TERMIOS.get() {
                unsafe {
                    libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, termios);
                }
            }
        }

        // Leave the alternate screen, disable common mouse modes, show cursor.
        write_to_fd(
            libc::STDERR_FILENO,
            b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1049l\x1b[?25h\r\n",
        );
    }

    fn write_report(bytes: &[u8]) {
        write_to_fd(libc::STDERR_FILENO, bytes);
        let fd = CRASH_LOG_FD.load(Ordering::Relaxed);
        if fd != libc::STDERR_FILENO {
            write_to_fd(fd, bytes);
        }
        if let Some(file) = CRASH_LOG.get() {
            if let Ok(mut file) = file.lock() {
                let _ = file.write_all(bytes);
                let _ = file.flush();
            }
        }
    }

    fn write_to_fd(fd: i32, bytes: &[u8]) {
        let mut written = 0;
        while written < bytes.len() {
            let rc = unsafe {
                libc::write(
                    fd,
                    bytes[written..].as_ptr().cast::<c_void>(),
                    bytes.len() - written,
                )
            };
            if rc <= 0 {
                break;
            }
            written += rc as usize;
        }
    }

    extern "C" fn signal_handler(
        signal: libc::c_int,
        _info: *mut libc::siginfo_t,
        _context: *mut c_void,
    ) {
        restore_terminal();

        let header = format!(
            "\nsequencer caught fatal signal {signal} ({})\ncrash log: {}\nstack trace:\n",
            signal_name(signal),
            crash_log_path()
        );
        write_report(header.as_bytes());

        let mut frames: [*mut c_void; 128] = [ptr::null_mut(); 128];
        let frame_count = unsafe { backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int) };
        let log_fd = CRASH_LOG_FD.load(Ordering::Relaxed);
        if frame_count > 0 {
            unsafe {
                backtrace_symbols_fd(frames.as_ptr(), frame_count, libc::STDERR_FILENO);
                if log_fd != libc::STDERR_FILENO {
                    backtrace_symbols_fd(frames.as_ptr(), frame_count, log_fd);
                }
            }
        }
        write_report(b"\n");

        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }

    fn signal_name(signal: libc::c_int) -> &'static str {
        match signal {
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGABRT => "SIGABRT",
            libc::SIGBUS => "SIGBUS",
            libc::SIGILL => "SIGILL",
            libc::SIGFPE => "SIGFPE",
            _ => "UNKNOWN",
        }
    }
}

#[cfg(not(unix))]
mod crash_handler {
    pub fn install(_log_fd: i32) {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            default_hook(panic_info);
        }));
    }
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
    let crash_log_path = std::env::var("TINYSEQ_CRASH_LOG")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "sequencer-crash.log".to_string());
    let _ = CRASH_LOG_PATH.set(crash_log_path.clone());
    let crash_log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&crash_log_path)?;
    #[cfg(unix)]
    let crash_log_fd = crash_log_file.as_raw_fd();
    #[cfg(not(unix))]
    let crash_log_fd = -1;
    let _ = CRASH_LOG.set(Mutex::new(crash_log_file));
    crash_handler::install(crash_log_fd);

    // Ensure samples/, effects/, and instruments/ directories exist
    std::fs::create_dir_all("samples").ok();
    std::fs::create_dir_all("effects").ok();
    std::fs::create_dir_all("instruments").ok();

    let engine::Engine {
        state,
        lg_ptr,
        buses,
        sample_rate,
        channels: _,
        master_recorder,
        keyboard_tx,
        _stream: stream,
    } = engine::init_engine()?;
    let lg_raw = lg_ptr.0;

    let mut app = ui::App::new(
        state.clone(),
        lg_ptr,
        sample_rate,
        buses,
        master_recorder,
        keyboard_tx,
    );

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--headless-custom-repro") {
        run_headless_custom_repro(&mut app)?;
        drop(stream);
        unsafe {
            sequencer::audiograph::clear_os_workgroup();
            sequencer::audiograph::engine_stop_workers();
            sequencer::audiograph::destroy_live_graph(lg_raw);
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
        sequencer::audiograph::clear_os_workgroup();
        sequencer::audiograph::engine_stop_workers();
        sequencer::audiograph::destroy_live_graph(lg_raw);
    }

    Ok(())
}

fn run_headless_custom_repro(app: &mut ui::App) -> Result<(), Box<dyn std::error::Error>> {
    let instrument_names = sequencer::lisp_effect::list_saved_instruments();
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
