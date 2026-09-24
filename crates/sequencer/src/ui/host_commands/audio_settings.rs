//! Settings > Audio (`content/ui/settings.lisp`): the saved audiograph worker
//! count (eseq-6jr2). The engine reads it once at start, so a change is saved
//! for next launch and the modal shows what is running now beside it.

use crate::*;
use sequencer::audio::worker_prefs::{self, WorkerPrefs};

pub(super) const COMMANDS: &[&str] = &["audio-set-workers"];

pub(crate) fn register_state(runtime: &mut eseqlisp::Runtime) {
    runtime.register_reactive("AUDIO", vec![
        ("workers-choice", Value::String(String::new())),
        ("workers-options", Value::List(vec![])),
        ("workers-note", Value::String(String::new())),
    ], true); // Presentation only; capture fixtures may seed a preview.
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    if name != "audio-set-workers" {
        return;
    }
    let choice = match payload {
        Value::Map(ref map) => map_string(map, "choice").unwrap_or_default(),
        _ => String::new(),
    };
    let auto = worker_prefs::auto_worker_count();
    let error = match parse_choice(&choice, auto) {
        Some(prefs) => worker_prefs::save(prefs).err(),
        None => Some(format!("Unrecognized worker choice {choice:?}")),
    };
    publish(editor, error);
}

/// Refresh the Audio section from disk; the modal calls this on open.
pub(crate) fn publish(editor: &mut Editor, error: Option<String>) {
    let prefs = worker_prefs::load();
    let auto = worker_prefs::auto_worker_count();
    let options = choice_options(auto, worker_prefs::logical_cores())
        .into_iter()
        .map(|option| Rc::new(RefCell::new(Value::String(option))))
        .collect();
    let note = error.unwrap_or_else(|| {
        workers_note(
            worker_prefs::resolve(prefs, auto),
            worker_prefs::running_worker_count(),
            worker_prefs::env_override(),
        )
    });
    let runtime = editor.runtime_mut();
    runtime.set_reactive("AUDIO", "workers-choice", Value::String(choice_label(prefs, auto)));
    runtime.set_reactive("AUDIO", "workers-options", Value::List(options));
    runtime.set_reactive("AUDIO", "workers-note", Value::String(note));
    editor.mark_needs_redraw();
}

fn auto_label(auto: u32) -> String {
    format!("Auto ({auto})")
}

fn choice_label(prefs: WorkerPrefs, auto: u32) -> String {
    prefs.workers.map_or_else(|| auto_label(auto), |n| n.to_string())
}

/// Auto first, then every count up to the logical cores (at least up to the
/// saved/auto value, so the current choice is always listed).
fn choice_options(auto: u32, logical: u32) -> Vec<String> {
    let top = logical.max(auto).min(worker_prefs::MAX_WORKERS);
    std::iter::once(auto_label(auto))
        .chain((1..=top).map(|n| n.to_string()))
        .collect()
}

fn parse_choice(choice: &str, auto: u32) -> Option<WorkerPrefs> {
    let choice = choice.trim();
    if choice == auto_label(auto) || choice.eq_ignore_ascii_case("auto") {
        return Some(WorkerPrefs { workers: None });
    }
    let workers = choice.parse::<u32>().ok()?;
    (workers <= worker_prefs::MAX_WORKERS).then_some(WorkerPrefs { workers: Some(workers) })
}

fn workers_note(next: u32, running: Option<u32>, env: Option<u32>) -> String {
    let plural = |n: u32| if n == 1 { "worker" } else { "workers" };
    if let Some(env) = env {
        return format!(
            "{} is set to {env}, which overrides this setting.",
            worker_prefs::ENV_OVERRIDE
        );
    }
    match running {
        Some(running) if running == next => {
            format!("Running {running} {}.", plural(running))
        }
        Some(running) => format!(
            "Running {running} {} now; {next} after you restart ESeq.",
            plural(running)
        ),
        None => format!("{next} {} at next launch.", plural(next)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_through_labels() {
        let auto = 6;
        let options = choice_options(auto, 10);
        assert_eq!(options.first().map(String::as_str), Some("Auto (6)"));
        assert_eq!(options.last().map(String::as_str), Some("10"));
        for option in &options {
            let prefs = parse_choice(option, auto).unwrap();
            assert_eq!(&choice_label(prefs, auto), option);
        }
        assert_eq!(parse_choice("auto", auto), Some(WorkerPrefs { workers: None }));
        assert_eq!(parse_choice("lots", auto), None);
        assert_eq!(parse_choice("9999", auto), None);
    }

    #[test]
    fn options_always_include_auto_count_on_small_machines() {
        assert_eq!(choice_options(4, 2), vec!["Auto (4)", "1", "2", "3", "4"]);
    }

    #[test]
    fn note_says_when_a_restart_is_needed() {
        assert_eq!(workers_note(6, Some(6), None), "Running 6 workers.");
        assert_eq!(
            workers_note(8, Some(6), None),
            "Running 6 workers now; 8 after you restart ESeq."
        );
        assert_eq!(workers_note(1, None, None), "1 worker at next launch.");
        assert!(workers_note(8, Some(3), Some(3)).contains("overrides"));
    }
}
