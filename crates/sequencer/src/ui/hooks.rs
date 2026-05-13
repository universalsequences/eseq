use std::sync::atomic::Ordering;
use std::time::Instant;

use eseqlisp::vm::format_lisp_value;
use eseqlisp::Editor as LispEditor;

use super::{App, HookCallback, HookUnit, PendingHookInvocation, SequencerHook};

const MAX_PENDING_HOOK_INVOCATIONS: usize = 128;
const MAX_HOOKS_PER_TICK: usize = 4;

fn unit_divisor(unit: HookUnit) -> u64 {
    match unit {
        HookUnit::Step => 1,
        HookUnit::Beat => 4,
        HookUnit::Bar => 16,
    }
}

fn hook_should_fire(unit: HookUnit, interval: u64, step_16th: u64) -> bool {
    let divisor = unit_divisor(unit);
    if divisor == 0 || interval == 0 || step_16th % divisor != 0 {
        return false;
    }
    let tick = step_16th / divisor;
    tick % interval == 0
}

impl App {
    pub fn tick_control_hooks(&mut self) {
        self.enqueue_due_hooks();
        self.run_pending_hooks();
    }

    pub fn tick_control_hooks_with_editor(&mut self, editor: &mut LispEditor) {
        self.enqueue_due_hooks();
        self.run_pending_hooks_with_editor(editor);
    }

    pub fn register_control_hook(
        &mut self,
        unit: HookUnit,
        interval: u64,
        track: usize,
        callback: HookCallback,
    ) -> String {
        let id = self.editor.next_hook_id;
        self.editor.next_hook_id += 1;
        self.editor.hooks.push(SequencerHook {
            id,
            unit,
            interval: interval.max(1),
            track,
            callback,
        });
        format!("Registered hook #{id}")
    }

    pub fn clear_control_hooks(&mut self) -> String {
        self.editor.hooks.clear();
        self.editor.pending_hook_invocations.clear();
        "Cleared hooks".to_string()
    }

    fn enqueue_due_hooks(&mut self) {
        let current = self.state.transport.playhead.load(Ordering::Relaxed) as u64;
        if !self.state.is_playing() {
            self.editor.last_hook_step_16th = Some(current);
            return;
        }

        let last = self
            .editor
            .last_hook_step_16th
            .unwrap_or(current.saturating_sub(1));
        if current <= last {
            self.editor.last_hook_step_16th = Some(current);
            return;
        }

        for step_16th in (last + 1)..=current {
            for hook in &self.editor.hooks {
                if hook_should_fire(hook.unit, hook.interval, step_16th) {
                    if self.editor.pending_hook_invocations.len() >= MAX_PENDING_HOOK_INVOCATIONS {
                        self.editor.pending_hook_invocations.pop_front();
                    }
                    self.editor
                        .pending_hook_invocations
                        .push_back(PendingHookInvocation {
                            hook_id: hook.id,
                            track: hook.track,
                            step_16th,
                            code: match &hook.callback {
                                HookCallback::Source(code) => code.clone(),
                                HookCallback::Global(name) => format!("({name})"),
                            },
                        });
                }
            }
        }

        self.editor.last_hook_step_16th = Some(current);
    }

    fn run_pending_hooks(&mut self) {
        let Some(runtime) = self.editor.scratch_runtime.as_mut() else {
            self.editor.pending_hook_invocations.clear();
            return;
        };

        for _ in 0..MAX_HOOKS_PER_TICK {
            let Some(invocation) = self.editor.pending_hook_invocations.pop_front() else {
                break;
            };
            let local_step = if invocation.track < self.tracks.len() {
                let num_steps = self.state.pattern.track_params[invocation.track]
                    .get_num_steps()
                    .max(1);
                (invocation.step_16th as usize) % num_steps
            } else {
                0
            };

            runtime.set_position(invocation.track, local_step);
            match runtime.eval(&invocation.code) {
                Ok(Some(value)) => {
                    if let Some(status) = runtime.take_status_message() {
                        self.editor.status_message = Some((status, Instant::now()));
                    } else {
                        self.editor.status_message = Some((
                            format!(
                                "Hook #{} => {}",
                                invocation.hook_id,
                                format_lisp_value(&value)
                            ),
                            Instant::now(),
                        ));
                    }
                }
                Ok(None) => {
                    if let Some(status) = runtime.take_status_message() {
                        self.editor.status_message = Some((status, Instant::now()));
                    }
                }
                Err(error) => {
                    self.editor.status_message = Some((
                        format!("Hook #{} error: {error}", invocation.hook_id),
                        Instant::now(),
                    ));
                }
            }
        }
    }

    fn run_pending_hooks_with_editor(&mut self, editor: &mut LispEditor) {
        for _ in 0..MAX_HOOKS_PER_TICK {
            let Some(invocation) = self.editor.pending_hook_invocations.pop_front() else {
                break;
            };
            let local_step = if invocation.track < self.tracks.len() {
                let num_steps = self.state.pattern.track_params[invocation.track]
                    .get_num_steps()
                    .max(1);
                (invocation.step_16th as usize) % num_steps
            } else {
                0
            };

            let runtime = editor.runtime_mut();
            let _ = runtime.eval_str(&format!(
                "(__host-set-current-track {})",
                invocation.track + 1
            ));
            let _ = runtime.eval_str(&format!("(__host-set-current-step {})", local_step + 1));

            match runtime.eval_str(&invocation.code) {
                Ok(Some(value)) => {
                    if let Some(status) = runtime.take_status_message() {
                        self.editor.status_message = Some((status, Instant::now()));
                    } else {
                        self.editor.status_message = Some((
                            format!(
                                "Hook #{} => {}",
                                invocation.hook_id,
                                format_lisp_value(&value)
                            ),
                            Instant::now(),
                        ));
                    }
                }
                Ok(None) => {
                    if let Some(status) = runtime.take_status_message() {
                        self.editor.status_message = Some((status, Instant::now()));
                    }
                }
                Err(error) => {
                    self.editor.status_message = Some((
                        format!("Hook #{} error: {error:?}", invocation.hook_id),
                        Instant::now(),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hook_should_fire, HookUnit};

    #[test]
    fn step_hooks_fire_on_matching_interval() {
        assert!(hook_should_fire(HookUnit::Step, 2, 4));
        assert!(!hook_should_fire(HookUnit::Step, 2, 5));
    }

    #[test]
    fn beat_hooks_fire_only_on_beat_boundaries() {
        assert!(hook_should_fire(HookUnit::Beat, 1, 8));
        assert!(!hook_should_fire(HookUnit::Beat, 1, 9));
    }

    #[test]
    fn bar_hooks_respect_interval() {
        assert!(hook_should_fire(HookUnit::Bar, 2, 32));
        assert!(!hook_should_fire(HookUnit::Bar, 2, 16));
    }
}
