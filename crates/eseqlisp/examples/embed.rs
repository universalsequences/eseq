use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use eseqlisp::frame;
use eseqlisp::tui;
use eseqlisp::vm::Value;
use eseqlisp::{CompileKind, Editor, EditorConfig, HostCommand, HostEvent, Runtime};

struct DemoHost {
    steps: Vec<bool>,
    pending_jobs: Vec<PendingJob>,
}

struct PendingJob {
    kind: CompileKind,
    name: Option<String>,
    ready_at: Instant,
}

fn main() -> std::io::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let init_src = std::fs::read_to_string("init.lisp").unwrap_or_default();
    let host = Rc::new(RefCell::new(DemoHost {
        steps: vec![false; 16],
        pending_jobs: Vec::new(),
    }));

    let mut runtime = Runtime::with_init_source(init_src);
    register_demo_natives(&mut runtime, host.clone());

    let mut editor = Editor::new(runtime, EditorConfig::default());
    let _ = editor.open_scratch_buffer("*host-demo*", demo_buffer_text());

    let mut terminal = ratatui::init();

    loop {
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => editor.handle_key(key),
                Event::Resize(_, _) => editor.mark_needs_redraw(),
                _ => {}
            }
        }

        process_host_commands(&mut editor, &host);
        process_pending_jobs(&mut editor, &host);

        if editor.needs_redraw() {
            terminal.draw(|f| {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                let viewport_width = (cols as usize).saturating_sub(2);
                let viewport_height = (rows as usize).saturating_sub(3);
                let render_frame =
                    frame::build_render_frame(&mut editor, viewport_width, viewport_height);
                tui::render(f, &render_frame);
            })?;
            editor.clear_needs_redraw();
        }

        if editor.should_quit() {
            break;
        }
    }

    ratatui::restore();
    Ok(())
}

fn register_demo_natives(runtime: &mut Runtime, host: Rc<RefCell<DemoHost>>) {
    runtime.register_native("seq-current-track", |_args, _ctx| Ok(Value::Number(1.0)));

    runtime.register_native("seq-num-steps", {
        let host = host.clone();
        move |_args, _ctx| Ok(Value::Number(host.borrow().steps.len() as f64))
    });

    runtime.register_native("seq-toggle-step", {
        let host = host.clone();
        move |args, ctx| {
            let idx = parse_step_index(&args, 0)?;
            let mut host = host.borrow_mut();
            let Some(step) = host.steps.get_mut(idx) else {
                return Err(format!("step {idx} out of range"));
            };
            *step = !*step;
            ctx.set_status(format!(
                "track 1 step {} -> {}",
                idx + 1,
                if *step { "on" } else { "off" }
            ));
            Ok(Value::Bool(*step))
        }
    });

    runtime.register_native("seq-clear-track", {
        let host = host.clone();
        move |_args, ctx| {
            let mut host = host.borrow_mut();
            for step in &mut host.steps {
                *step = false;
            }
            ctx.set_status("track 1 cleared");
            Ok(Value::Bool(true))
        }
    });

    runtime.register_native("seq-dump-track", move |_args, _ctx| {
        let rendered = host
            .borrow()
            .steps
            .iter()
            .enumerate()
            .map(|(idx, on)| format!("{}:{}", idx + 1, if *on { "x" } else { "." }))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(Value::String(rendered))
    });
}

fn process_host_commands(editor: &mut Editor, host: &Rc<RefCell<DemoHost>>) {
    for command in editor.drain_host_commands() {
        match command {
            HostCommand::CompileInstrument {
                suggested_name,
                path,
                ..
            } => {
                let label = suggested_name
                    .clone()
                    .or_else(|| {
                        path.and_then(|path| {
                            path.file_stem()
                                .map(|stem| stem.to_string_lossy().to_string())
                        })
                    })
                    .unwrap_or_else(|| "untitled".to_string());
                editor.handle_host_event(HostEvent::CommandStarted {
                    label: format!("compile instrument '{label}'"),
                });
                host.borrow_mut().pending_jobs.push(PendingJob {
                    kind: CompileKind::Instrument,
                    name: Some(label),
                    ready_at: Instant::now() + Duration::from_millis(3000),
                });
            }
            HostCommand::CompileEffect {
                suggested_name,
                path,
                ..
            } => {
                let label = suggested_name
                    .clone()
                    .or_else(|| {
                        path.and_then(|path| {
                            path.file_stem()
                                .map(|stem| stem.to_string_lossy().to_string())
                        })
                    })
                    .unwrap_or_else(|| "untitled".to_string());
                editor.handle_host_event(HostEvent::CommandStarted {
                    label: format!("compile effect '{label}'"),
                });
                host.borrow_mut().pending_jobs.push(PendingJob {
                    kind: CompileKind::Effect,
                    name: Some(label),
                    ready_at: Instant::now() + Duration::from_millis(2500),
                });
            }
            HostCommand::Custom { name, payload } => {
                if name == "compile-current" {
                    let label = extract_name_from_payload(&payload)
                        .unwrap_or_else(|| "untitled".to_string());
                    editor.handle_host_event(HostEvent::CommandStarted {
                        label: format!("compile instrument '{label}'"),
                    });
                    host.borrow_mut().pending_jobs.push(PendingJob {
                        kind: CompileKind::Instrument,
                        name: Some(label),
                        ready_at: Instant::now() + Duration::from_millis(3000),
                    });
                } else {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "host command '{name}' queued with payload {payload:?}"
                    )));
                }
            }
        }
    }
}

fn process_pending_jobs(editor: &mut Editor, host: &Rc<RefCell<DemoHost>>) {
    let now = Instant::now();
    let mut ready = Vec::new();
    {
        let mut host = host.borrow_mut();
        let mut idx = 0;
        while idx < host.pending_jobs.len() {
            if host.pending_jobs[idx].ready_at <= now {
                ready.push(host.pending_jobs.remove(idx));
            } else {
                idx += 1;
            }
        }
    }

    for job in ready {
        editor.handle_host_event(HostEvent::CompileFinished {
            kind: job.kind,
            success: true,
            name: job.name,
            diagnostics: None,
        });
    }
}

fn parse_step_index(args: &[Value], arg_idx: usize) -> Result<usize, String> {
    let Some(Value::Number(step)) = args.get(arg_idx) else {
        return Err("expected step number".to_string());
    };
    let step = *step as isize;
    if step <= 0 {
        return Err("steps are 1-based".to_string());
    }
    Ok((step - 1) as usize)
}

fn demo_buffer_text() -> &'static str {
    r#"; Interactive embedding demo for eseqlisp.
; Keys:
;   C-x C-e  eval sexp at cursor
;   C-x C-b  eval buffer
;   C-x C-s  save buffer
;   C-c C-k  simulate compile instrument
;   C-c C-c  simulate compile effect
;   C-q      quit
;
; Host-provided natives in this demo:
;   (seq-current-track)
;   (seq-num-steps)
;   (seq-toggle-step 1)
;   (seq-clear-track)
;   (seq-dump-track)

(status "Host demo ready")

(seq-current-track)
(seq-num-steps)
(seq-toggle-step 1)
(seq-toggle-step 5)
(seq-dump-track)

; Try this anywhere:
; (compile-instrument)
; (compile-effect)
"#
}

fn extract_name_from_payload(payload: &Value) -> Option<String> {
    let Value::Map(map) = payload else {
        return None;
    };
    let value = map.get("name").or_else(|| map.get("suggested-name"))?;
    match &*value.borrow() {
        Value::String(name) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}
