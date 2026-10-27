//! Script evaluations that wait on finite-element solves. In the app,
//! `modal-solve` never solves a solid that is not cached yet
//! ([`sequencer::lisp_host::deferring_solves`]): the evaluation stops, the
//! solids it asked for are solved here on a worker thread (their progress in
//! the status line), and then what evaluated the script runs again, reading
//! them from the cache. A solve of minutes never blocks the event loop.
//!
//! One evaluation waits at a time: a newer one cancels the older, unless it
//! waits on the same solids (C-c C-c pressed again), which keeps solving.

use crate::*;
use sequencer::lisp_host::DeferredSolve;
use sequencer::modal_fe::{SolvePhase, SolveProgress};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

pub(super) const COMMANDS: &[&str] = &[RESUME];

/// Queued when the solves finish, so the evaluation runs again with the
/// whole loop context.
const RESUME: &str = "script-solves-resume";

/// What evaluated the script, to run again once its solids are cached.
pub(crate) enum Resume {
    /// C-c C-c on the script buffer named.
    Evaluate(String),
    /// The shape lab's load (its payload).
    LabLoad(Value),
    /// The shape lab's Update draft of the script buffer named (its
    /// write-back is done; only the evaluation is left).
    LabApply(String),
}

struct Running {
    resume: Resume,
    /// The cache files of the solids solving.
    keys: Vec<PathBuf>,
    /// Those solved earlier for the same evaluation (see [`ScriptSolves`]).
    solved_before: Vec<PathBuf>,
    /// Which of the solids is solving.
    current: Arc<AtomicUsize>,
    progress: Arc<SolveProgress>,
    /// The progress last shown, in whole percent.
    shown: Option<(usize, SolvePhase, u32)>,
    receiver: mpsc::Receiver<Result<(), String>>,
}

#[derive(Default)]
pub(crate) struct ScriptSolves {
    running: Option<Running>,
    /// Solved, waiting for [`RESUME`] to run it again, with the cache files
    /// of every solid solved for it so far.
    finished: Option<(Resume, Vec<PathBuf>)>,
    /// While an evaluation runs again, those cache files: a solid it asks
    /// for again did not reach the cache, which fails it rather than solving
    /// forever.
    resuming: Option<Vec<PathBuf>>,
}

impl ScriptSolves {
    /// Whether solves are running or an evaluation waits to run again (the
    /// event loop keeps ticking).
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || self.finished.is_some()
    }
}

/// Solve `deferred` in the background, then run `resume` again. The status
/// line names what is solving.
pub(crate) fn start(
    editor: &mut Editor,
    solves: &mut ScriptSolves,
    resume: Resume,
    deferred: Vec<DeferredSolve>,
) -> Result<(), String> {
    let keys: Vec<PathBuf> = deferred.iter().map(|solve| solve.cache.clone()).collect();
    let solved_before = solves.resuming.clone().unwrap_or_default();
    if keys.iter().any(|key| solved_before.contains(key)) {
        return Err("a solved solid did not reach the modal solve cache (is its folder writable?)".into());
    }
    solves.finished = None;
    if let Some(running) = solves.running.as_mut() {
        if running.keys == keys {
            running.resume = resume;
            return Ok(());
        }
        running.progress.cancel();
    }
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(SolveProgress::default());
    let current = Arc::new(AtomicUsize::new(0));
    let (watched, solving) = (progress.clone(), current.clone());
    std::thread::spawn(move || {
        // a solid that fails to solve fails its call when the script runs
        // again, as a solve on the spot did; only a cancel stops the rest
        let result = deferred.iter().enumerate().try_for_each(|(index, solve)| {
            solving.store(index, Ordering::Relaxed);
            match sequencer::lisp_host::solve_solid_cached(&solve.spec, &solve.base, &watched) {
                Err(error) if watched.is_cancelled() => Err(error),
                _ => Ok(()),
            }
        });
        let _ = sender.send(result);
    });
    editor.handle_host_event(HostEvent::Status(format!("Solving {}…", solids(keys.len()))));
    solves.running = Some(Running { resume, keys, solved_before, current, progress, shown: None, receiver });
    Ok(())
}

fn solids(count: usize) -> String {
    if count == 1 { "a modal solid".into() } else { format!("{count} modal solids") }
}

/// Show the running solves' progress (when it moved a percent); once they
/// are done, queue the evaluation to run again, or report their failure.
pub(crate) fn poll_script_solves(editor: &mut Editor, solves: &mut ScriptSolves) {
    let Some(running) = solves.running.as_mut() else { return };
    let outcome = match running.receiver.try_recv() {
        Ok(outcome) => outcome,
        Err(mpsc::TryRecvError::Empty) => {
            let index = running.current.load(Ordering::Relaxed);
            let (phase, done) = running.progress.snapshot();
            let shown = (index, phase, (done * 100.0) as u32);
            if running.shown != Some(shown) {
                running.shown = Some(shown);
                let which = match running.keys.len() {
                    1 => "Solving the modal solid".to_string(),
                    count => format!("Solving modal solid {} of {count}", index + 1),
                };
                editor.handle_host_event(HostEvent::Status(format!("{which}: {} {}%", phase.name(), shown.2)));
                editor.mark_needs_redraw();
            }
            return;
        }
        Err(mpsc::TryRecvError::Disconnected) => Err("the solver thread crashed".to_string()),
    };
    let Running { resume, keys, mut solved_before, .. } = solves.running.take().expect("running");
    match outcome {
        Ok(()) => {
            solved_before.extend(keys);
            solves.finished = Some((resume, solved_before));
            editor.runtime_mut().enqueue_host_command(HostCommand::Custom { name: RESUME.into(), payload: Value::Nil });
        }
        Err(error) => fail(editor, resume, error),
    }
    editor.mark_needs_redraw();
}

fn fail(editor: &mut Editor, resume: Resume, error: String) {
    match resume {
        Resume::Evaluate(_) | Resume::LabLoad(_) => editor_error(editor, format!("Solving: {error}")),
        Resume::LabApply(_) => super::shape_lab::draft_status(editor, "failed", Some(error)),
    }
}

pub(super) fn handle(name: &str, _payload: Value, app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    if name != RESUME {
        return;
    }
    let Some((resume, solved)) = ctx.sessions.script_solves.finished.take() else { return };
    ctx.sessions.script_solves.resuming = Some(solved);
    let outcome = match &resume {
        Resume::Evaluate(name) => super::drafts::evaluate_named(app, editor, ctx, name),
        Resume::LabLoad(payload) => super::shape_lab::load_or_solve(editor, &mut ctx.sessions.script_solves, payload),
        Resume::LabApply(name) => super::shape_lab::evaluate_for_lab(app, editor, ctx, name),
    };
    ctx.sessions.script_solves.resuming = None;
    if let Err(error) = outcome {
        fail(editor, resume, error);
    }
}
