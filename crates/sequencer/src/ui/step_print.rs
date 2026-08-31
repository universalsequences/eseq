/*!
Live parameter printing (beads eseq-jc9 and eseq-prm).

While transport and recording are active, touching an eligible parameter
latches a [`PrintTarget`] and writes its value onto every triggered step the
playhead passes. The hold ends on control release; transport/recording-off or
a focused-track change also disarms it. Multiple held targets print together.

Step targets write live `step_data`, use the engine-side override so the
write is audible before snapshot republish, and push targeted step
invalidations. Track instrument/effect targets write atomic p-lock data the
audio thread reads directly. MIDI-FX and rack targets are coalesced into one
scheduler-track publish per tick (the scheduler reads both families from its
published snapshot), and bus targets into one bus-runtime publish. Base values
remain untouched throughout. Every path rides the open `RecordingHistoryTransaction`,
so one record pass undoes as one "Record take" entry rather than creating
device-edit gesture history.
*/

use super::*;
use sequencer::sequencer::RollHitRecorded;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrintTarget {
    Step(StepParam),
    Instrument { param_idx: usize },
    Effect { slot_idx: usize, param_idx: usize },
    BusEffect { bus_idx: usize, slot_idx: usize, param_idx: usize },
    MidiFx { slot_idx: usize, param_idx: usize },
    RackSlotParam { slot_idx: usize, param: RackSlotParam },
    RackSlotInstrument { slot_idx: usize, param_idx: usize },
    RackSlotEffect {
        rack_slot_idx: usize,
        effect_slot_idx: usize,
        param_idx: usize,
    },
    RackMacro { macro_idx: usize },
}

impl From<StepParam> for PrintTarget {
    fn from(param: StepParam) -> Self {
        Self::Step(param)
    }
}

#[derive(Default)]
pub(crate) struct StepPrintState {
    /// Track the latch belongs to (the focused track at touch time).
    track: usize,
    /// Latched (target, value) pairs — only touched params print.
    values: Vec<(PrintTarget, f32)>,
    /// Playhead step printed last tick, so each boundary prints once and a
    /// frame that skipped steps can catch up across the gap (wrap-aware).
    last_step: Option<usize>,
    /// A latch/value change since the last tick: the step under the playhead
    /// reprints even without a boundary crossing, so the touch itself lands
    /// on the current step.
    touch_dirty: bool,
    /// Tracks (bitmask) whose printed values are in live pattern state but
    /// not yet in the scheduler snapshot. Per-track so the flush can use
    /// copy-on-write `publish_scheduler_track` publishes — a full snapshot
    /// capture per printing frame starved the (normal-priority) scheduler
    /// thread on weak machines and made the whole track's timing shaky.
    dirty_unpublished_tracks: u64,
}

/// One frame's print result: which steps on which track took which params.
#[derive(Default)]
pub(crate) struct PrintedSteps {
    pub(crate) track: usize,
    pub(crate) steps: Vec<usize>,
    pub(crate) targets: Vec<(PrintTarget, f32)>,
}

impl StepPrintState {
    pub(crate) fn armed(&self) -> bool {
        !self.values.is_empty()
    }

    /// Arm-on-touch: latch a param value for printing. A touch on a
    /// different track than the current latch restarts the latch there.
    pub(crate) fn latch(
        &mut self,
        track: usize,
        target: impl Into<PrintTarget>,
        value: f32,
    ) {
        let target = target.into();
        if self.armed() && self.track != track {
            self.disarm();
        }
        self.track = track;
        match self.values.iter_mut().find(|(existing, _)| *existing == target) {
            Some(entry) => entry.1 = value,
            None => self.values.push((target, value)),
        }
        self.touch_dirty = true;
    }

    /// End the print latch. Deliberately keeps `dirty_unpublished_tracks`:
    /// written steps still owe a snapshot publish even after the latch ends.
    pub(crate) fn disarm(&mut self) {
        self.track = 0;
        self.values.clear();
        self.last_step = None;
        self.touch_dirty = false;
    }

    /// Mouse-up on one picker ends that param's print (hold-to-print: the
    /// gesture is the arm). Returns whether the whole latch ended — the last
    /// held param releasing disarms print mode entirely.
    pub(crate) fn unlatch(&mut self, target: impl Into<PrintTarget>) -> bool {
        let target = target.into();
        self.values.retain(|(existing, _)| *existing != target);
        if self.values.is_empty() {
            self.disarm();
            return true;
        }
        false
    }

    /// Pointer release ends device-param print gestures. Step targets have
    /// their own picker-specific release command, so a release must not erase
    /// an unrelated step-param hold.
    pub(crate) fn release_device_param_gesture(&mut self) {
        self.values
            .retain(|(target, _)| matches!(target, PrintTarget::Step(_)));
        if self.values.is_empty() {
            self.disarm();
        }
    }

    /// Mirror the latch into the engine-side override so the scheduler plays
    /// the latched values on the armed track immediately (the pattern write
    /// lands behind the playhead, so without this the print would only be
    /// heard one loop later). Call after every latch change; `print_pass`
    /// clears the engine side on disarm.
    pub(crate) fn publish_engine_override(&self, state: &SequencerState) {
        if !self.armed() {
            state.step_print_override.clear();
            return;
        }
        let value = |param: StepParam| {
            self.values
                .iter()
                .find(|(target, _)| *target == PrintTarget::Step(param))
                .map(|(_, value)| *value)
        };
        let velocity = value(StepParam::Velocity);
        let duration = value(StepParam::Duration);
        let transpose = value(StepParam::Transpose);
        if velocity.is_none() && duration.is_none() && transpose.is_none() {
            state.step_print_override.clear();
        } else {
            state
                .step_print_override
                .set(self.track, velocity, duration, transpose);
        }
    }

    /// Rolled hits recorded while print mode is armed take the latched
    /// values at record time — only for touched params on the armed track.
    pub(crate) fn override_roll_hit(&self, hit: &mut RollHitRecorded) {
        if !self.armed() || hit.track != self.track {
            return;
        }
        for (target, value) in &self.values {
            match target {
                PrintTarget::Step(StepParam::Transpose) => hit.transpose = *value,
                PrintTarget::Step(StepParam::Velocity) => hit.velocity = *value,
                PrintTarget::Step(StepParam::Duration) => hit.duration_steps = *value,
                _ => {}
            }
        }
    }
}

/// Divert already-resolved, already-clamped base-value edits into the live
/// print latch when the shared record/play/no-selection gate is active.
/// Callers must preserve any target-specific higher-precedence diversion
/// (notably neural overrides) before reaching this helper.
pub(crate) fn try_latch_param_print(
    shared: &SharedHandles,
    track: usize,
    targets: &[(PrintTarget, f32)],
) -> bool {
    if targets.is_empty()
        || shared.current_track.load(Ordering::Relaxed) != track
        || !shared.selected_steps.lock().unwrap().is_empty()
        || !shared.state.is_playing()
        || !shared.recording.load(Ordering::Relaxed)
    {
        return false;
    }
    let mut print = shared.step_print.lock().unwrap();
    for (target, value) in targets {
        print.latch(track, *target, *value);
    }
    // A cross-track touch may have replaced a Step target, so clear or
    // republish its engine-only override at the same latch transition.
    print.publish_engine_override(&shared.state);
    true
}

/// One frame of print mode: disarm when the (recording && playing && same
/// focused track) gate fails, otherwise write the latched params onto every
/// trigger step the playhead has passed since the previous frame (walking
/// the wrap, so a loop boundary keeps printing). Pure state + pattern
/// mutation; invalidations/publishing live in `tick_step_print`.
pub(crate) fn print_pass(
    state: &SequencerState,
    print: &mut StepPrintState,
    focused_track: usize,
    recording: bool,
) -> PrintedSteps {
    if !print.armed() {
        return PrintedSteps::default();
    }
    if !recording
        || !state.is_playing()
        || print.track != focused_track
        || print.track >= state.pattern.patterns.len()
    {
        // Pause/stop, record-off, or a focus move ends printing cleanly —
        // the next touch during the next record+play session re-arms. The
        // engine-side override dies with the latch.
        print.disarm();
        state.step_print_override.clear();
        return PrintedSteps::default();
    }
    let track = print.track;
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .clamp(1, sequencer::sequencer::MAX_STEPS);
    let playhead =
        state.transport.track_playheads[track].load(Ordering::Relaxed) as usize % num_steps;
    let mut boundary_steps = Vec::new();
    match print.last_step {
        // First tick after the touch: print the step under the playhead.
        None => boundary_steps.push(playhead),
        Some(prev) => {
            if prev != playhead {
                // Walk (prev, playhead] modulo the track length so skipped
                // frames and the loop wrap both keep the latch printing.
                let mut step = (prev + 1) % num_steps;
                for _ in 0..num_steps {
                    boundary_steps.push(step);
                    if step == playhead {
                        break;
                    }
                    step = (step + 1) % num_steps;
                }
            } else if print.touch_dirty {
                // Value moved within the same step: reprint it.
                boundary_steps.push(playhead);
            }
        }
    }
    print.last_step = Some(playhead);
    print.touch_dirty = false;
    let mut printed = Vec::new();
    for step in boundary_steps {
        // Trigger steps only — printing never creates triggers.
        if !state.pattern.patterns[track].is_active(step) {
            continue;
        }
        for (target, value) in &print.values {
            if let PrintTarget::Step(param) = target {
                state.set_step_param_no_publish(track, step, *param, *value);
            }
        }
        printed.push(step);
    }
    if printed.is_empty() {
        return PrintedSteps::default();
    }
    if print
        .values
        .iter()
        .any(|(target, _)| matches!(target, PrintTarget::Step(_)))
    {
        print.dirty_unpublished_tracks |=
            1u64 << track.min(sequencer::sequencer::MAX_TRACKS - 1);
    }
    PrintedSteps {
        track,
        steps: printed,
        targets: print.values.clone(),
    }
}

/// What one `tick_step_print` frame did, for the reactive tick to act on.
#[derive(Default)]
pub(crate) struct StepPrintTick {
    /// Steps were printed: mark the record take changed + redraw.
    pub(crate) printed: bool,
    /// The *step* panel's picker readouts changed: run a reactive cycle and
    /// refresh the *step* layout so the display tracks the latch tightly.
    pub(crate) display_dirty: bool,
}

/// While armed, the *step* panel's `fx-step-value-*` pickers must read the
/// LATCH — the value being printed — not the cursor step, or the readout
/// lags/snaps back mid-sweep. Re-asserted every armed frame (set_reactive is
/// change-detecting) so cursor-step republishes from other sync paths
/// self-heal within a frame.
fn sync_print_display_fields(rt: &mut Runtime, print: &StepPrintState) -> bool {
    let mut dirty = false;
    for (target, value) in &print.values {
        if let PrintTarget::Step(param) = target {
            if let Some(field) = fx_step_param_value_field(*param) {
                dirty |= rt
                    .set_reactive("SEQ", field, Value::Number(*value as f64))
                    .effects_dirty;
            }
        }
    }
    dirty
}

/// The latch just ended: hand the picker readouts back to the cursor step
/// (or the selected step, matching `sync_single_step_param_binding`).
pub(crate) fn restore_cursor_display_fields(
    rt: &mut Runtime,
    state: &SequencerState,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    if track >= state.pattern.step_data.len() {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .clamp(1, sequencer::sequencer::MAX_STEPS);
    let parameter_step = selected_plock_step(selected_steps)
        .unwrap_or_else(|| fx_step_cursor_from_runtime(rt))
        .min(num_steps.saturating_sub(1));
    let mut dirty = false;
    for param in [StepParam::Velocity, StepParam::Duration, StepParam::Transpose] {
        if let Some(field) = fx_step_param_value_field(param) {
            let value = state.pattern.step_data[track].get(parameter_step, param);
            dirty |= rt
                .set_reactive("SEQ", field, Value::Number(value as f64))
                .effects_dirty;
        }
    }
    dirty
}

/// Per-frame drive, mirroring `tick_roll_record`: run the print pass, push
/// targeted step invalidations for what landed (no `ui_epoch` bump — see the
/// `set-step-param-history` contract), and republish the scheduler snapshot
/// so the printed values are audible on the same pass's later steps. The
/// publish defers while a roll hold has unpublished pattern writes; the
/// roll's own release publish carries the printed values along.
pub(crate) fn tick_step_print(
    app: &mut app::App,
    shared: &SharedHandles,
    rt: &mut Runtime,
) -> StepPrintTick {
    let mut print = shared.step_print.lock().unwrap();
    if !print.armed() && print.dirty_unpublished_tracks == 0 {
        return StepPrintTick::default();
    }
    let was_armed = print.armed();
    let recording = shared.recording.load(Ordering::Relaxed);
    let focused_track = shared.current_track.load(Ordering::Relaxed);
    let printed = print_pass(&shared.state, &mut print, focused_track, recording);
    let display_dirty = if print.armed() {
        sync_print_display_fields(rt, &print)
    } else if was_armed {
        // Gate just failed: give the readouts back to the cursor step.
        restore_cursor_display_fields(rt, &shared.state, focused_track, &shared.selected_steps)
    } else {
        false
    };
    let roll_publish_pending = shared
        .roll_record
        .lock()
        .unwrap()
        .has_unpublished_writes();
    let publish_tracks = if roll_publish_pending {
        0
    } else {
        std::mem::take(&mut print.dirty_unpublished_tracks)
    };
    drop(print);
    // Copy-on-write per-track publishes: only the printed track changed, and
    // the latch is audible through the engine-side override anyway — the
    // publish just has to land before the latch ends. A full snapshot capture
    // here ran every printing frame and starved the scheduler thread.
    let mut remaining = publish_tracks;
    while remaining != 0 {
        let track = remaining.trailing_zeros() as usize;
        remaining &= remaining - 1;
        shared.state.publish_scheduler_track(track);
    }
    let mut wrote = false;
    let mut track_snapshot_dirty = false;
    let mut bus_runtime_dirty = false;
    let mut plock_presence_steps = Vec::new();
    for step in &printed.steps {
        let mut wrote_plock = false;
        for (target, value) in &printed.targets {
            match target {
                PrintTarget::Step(param) => {
                    wrote = true;
                    shared.ui_invalidations.push(UiInvalidation::Step {
                        track: printed.track,
                        step: *step,
                        change: StepInvalidation::Param((*param).into()),
                    });
                    if *param == StepParam::Duration {
                        shared.ui_invalidations.push(UiInvalidation::Step {
                            track: printed.track,
                            step: *step,
                            change: StepInvalidation::DurationSpan,
                        });
                    }
                }
                PrintTarget::Instrument { param_idx } => {
                    let target_wrote = app.print_instrument_plock(
                        printed.track,
                        *step,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                }
                PrintTarget::Effect { slot_idx, param_idx } => {
                    let target_wrote = app.print_effect_plock(
                        printed.track,
                        *step,
                        *slot_idx,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                }
                PrintTarget::BusEffect { bus_idx, slot_idx, param_idx } => {
                    let target_wrote = app.print_bus_effect_plock(
                        *bus_idx,
                        *step,
                        *slot_idx,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    bus_runtime_dirty |= target_wrote;
                }
                PrintTarget::MidiFx { slot_idx, param_idx } => {
                    let target_wrote = app.print_midi_fx_plock(
                        printed.track,
                        *step,
                        *slot_idx,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::RackSlotParam { slot_idx, param } => {
                    let target_wrote = app.print_rack_slot_param_plock(
                        printed.track,
                        *step,
                        *slot_idx,
                        *param,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::RackSlotInstrument { slot_idx, param_idx } => {
                    let target_wrote = app.print_rack_slot_instrument_plock(
                        printed.track,
                        *step,
                        *slot_idx,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::RackSlotEffect {
                    rack_slot_idx,
                    effect_slot_idx,
                    param_idx,
                } => {
                    let target_wrote = app.print_rack_slot_effect_plock(
                        printed.track,
                        *step,
                        *rack_slot_idx,
                        *effect_slot_idx,
                        *param_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::RackMacro { macro_idx } => {
                    let target_wrote = app.print_rack_macro_plock(
                        printed.track,
                        *step,
                        *macro_idx,
                        *value,
                    );
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
            }
        }
        if wrote_plock {
            plock_presence_steps.push(*step);
        }
    }
    // Rack and MIDI-FX p-locks reach the scheduler only through its published
    // per-track snapshot, while bus p-locks live in the separately published
    // bus runtime. Coalesce each publication once per tick after all writes
    // land; never publish once per target/step.
    if track_snapshot_dirty {
        shared.state.publish_scheduler_track(printed.track);
    }
    if bus_runtime_dirty {
        app.publish_bus_effect_runtime();
        *shared.bus_state.lock().unwrap() = app.buses.clone();
    }
    // A held batch can contain several params and a slow UI frame can cross
    // several steps. Publish one track-scoped presence batch after every
    // p-lock write has landed, rather than invalidating per target (or
    // widening the update through ui_epoch/fx_epoch).
    if !plock_presence_steps.is_empty() {
        shared
            .ui_invalidations
            .push(UiInvalidation::StepInvalidationBatch {
                track: printed.track,
                steps: plock_presence_steps,
                change: StepInvalidation::PlockPresence,
            });
    }
    StepPrintTick {
        printed: wrote,
        display_dirty,
    }
}

#[cfg(test)]
mod step_print_tests {
    use super::{
        print_pass, restore_cursor_display_fields, sync_print_display_fields, PrintTarget,
        StepPrintState,
    };
    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;
    use sequencer::sequencer::{
        default_empty_effect_chain, RollHitRecorded, SequencerState, StepParam,
    };
    use std::collections::HashSet;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    fn playing_state() -> SequencerState {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.transport.playing.store(true, Ordering::Relaxed);
        state
    }

    fn set_playhead(state: &SequencerState, step: usize) {
        state.transport.track_playheads[0].store(step as u32, Ordering::Relaxed);
    }

    #[test]
    fn playback_and_record_alone_change_nothing_until_a_param_is_touched() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
        let mut print = StepPrintState::default();
        let printed = print_pass(&state, &mut print, 0, true);
        assert!(printed.steps.is_empty());
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Velocity), before);
    }

    #[test]
    fn touch_prints_the_step_under_the_playhead_and_only_the_touched_param() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(4);
        set_playhead(&state, 4);
        let duration_before = state.pattern.step_data[0].get(4, StepParam::Duration);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.25);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![4]);
        assert_eq!(state.pattern.step_data[0].get(4, StepParam::Velocity), 0.25);
        // Untouched params on the printed step are left alone.
        assert_eq!(
            state.pattern.step_data[0].get(4, StepParam::Duration),
            duration_before
        );
    }

    #[test]
    fn print_updates_chord_backed_sounding_params() {
        let state = playing_state();
        let step = 4;
        state.pattern.patterns[0].toggle_step(step);
        state.pattern.step_data[0].set(step, StepParam::Transpose, 7.0);
        state.pattern.step_data[0].set(step, StepParam::Duration, 2.0);
        state.pattern.chord_data[0].add_note_with_duration(step, 7.0, 2.0);
        set_playhead(&state, step);

        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Transpose, 12.0);
        print.latch(0, StepParam::Duration, 3.5);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![step]);

        assert_eq!(state.pattern.chord_data[0].get(step, 0), 12.0);
        assert_eq!(state.pattern.chord_data[0].get_duration(step, 0), 3.5);
    }

    #[test]
    fn latch_keeps_printing_across_the_loop_wrap_and_skips_trigger_less_steps() {
        let state = playing_state();
        // Triggers on 14 and 1; 15 and 0 stay empty across the wrap.
        state.pattern.patterns[0].toggle_step(14);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 13);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Duration, 3.0);
        let printed = print_pass(&state, &mut print, 0, true);
        assert!(printed.steps.is_empty(), "step 13 has no trigger");
        // Frame gap: playhead moved 13 -> 1 across the wrap; the latch (not
        // a fresh touch) prints every passed trigger step.
        set_playhead(&state, 1);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![14, 1]);
        assert_eq!(state.pattern.step_data[0].get(14, StepParam::Duration), 3.0);
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Duration), 3.0);
        // Trigger-less steps stay at their defaults.
        let default_duration = StepParam::Duration.default_value();
        assert_eq!(
            state.pattern.step_data[0].get(15, StepParam::Duration),
            default_duration
        );
        assert_eq!(
            state.pattern.step_data[0].get(0, StepParam::Duration),
            default_duration
        );
    }

    #[test]
    fn value_moves_within_one_step_reprint_it_but_a_still_latch_does_not_rewrite() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(2);
        set_playhead(&state, 2);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![2]);
        // Playhead still on 2, no new touch: nothing to write.
        assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        // The picker moved: the same step takes the new value.
        print.latch(0, StepParam::Velocity, 0.75);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![2]);
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Velocity), 0.75);
    }

    #[test]
    fn pause_record_off_and_focus_change_each_disarm_without_stray_writes() {
        for (playing, recording, focused_track) in [(false, true, 0), (true, false, 0), (true, true, 1)] {
            let state = playing_state();
            state.transport.playing.store(playing, Ordering::Relaxed);
            state.pattern.patterns[0].toggle_step(0);
            set_playhead(&state, 0);
            let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
            let mut print = StepPrintState::default();
            print.latch(0, StepParam::Velocity, 0.1);
            let printed = print_pass(&state, &mut print, focused_track, recording);
            assert!(printed.steps.is_empty());
            assert!(!print.armed(), "gate ({playing}, {recording}, track {focused_track}) must disarm");
            assert_eq!(state.pattern.step_data[0].get(0, StepParam::Velocity), before);
            // Disarmed: the next pass with the gate restored stays silent
            // until the next touch (re-arm on touch, not on transport).
            state.transport.playing.store(true, Ordering::Relaxed);
            assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        }
    }

    #[test]
    fn rolled_hits_take_the_latched_values_for_touched_params_only() {
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Duration, 2.5);
        print.latch(0, StepParam::Velocity, 0.6);
        let mut hit = RollHitRecorded {
            track: 0,
            step: 3,
            delay: 0.0,
            transpose: 7.0,
            velocity: 1.0,
            duration_steps: 0.25,
            beat: 1.5,
        };
        print.override_roll_hit(&mut hit);
        assert_eq!(hit.duration_steps, 2.5);
        assert_eq!(hit.velocity, 0.6);
        // Transpose untouched: the rolled key's pitch survives.
        assert_eq!(hit.transpose, 7.0);
        // Hits on other tracks are never overridden.
        let mut other = RollHitRecorded { track: 1, ..hit };
        let velocity_before = other.velocity;
        print.override_roll_hit(&mut other);
        assert_eq!(other.velocity, velocity_before);
    }

    #[test]
    fn instrument_targets_share_the_boundary_latch_without_mutating_step_values() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(3);
        state.pattern.instrument_slots[0].defaults.set(2, 0.2);
        set_playhead(&state, 3);
        let mut print = StepPrintState::default();
        let target = PrintTarget::Instrument { param_idx: 2 };
        print.latch(0, target, 0.8);

        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![3]);
        assert_eq!(printed.targets, vec![(target, 0.8)]);
        assert_eq!(state.pattern.instrument_slots[0].defaults.get(2), 0.2);
        assert_eq!(state.pattern.instrument_slots[0].plocks.get(3, 2), None);
        assert_eq!(
            state.step_print_override.values_for_track(0),
            (None, None, None),
            "instrument printing must not use the step engine override"
        );

        print.release_device_param_gesture();
        assert!(!print.armed());
    }

    #[test]
    fn device_gesture_release_removes_instrument_and_effect_targets_only() {
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        print.latch(0, PrintTarget::Instrument { param_idx: 2 }, 0.6);
        print.latch(
            0,
            PrintTarget::Effect {
                slot_idx: 1,
                param_idx: 3,
            },
            0.7,
        );

        print.release_device_param_gesture();

        assert!(print.armed(), "the independent step-param hold remains armed");
        assert_eq!(
            print.values,
            vec![(PrintTarget::Step(StepParam::Velocity), 0.5)]
        );
    }

    #[test]
    fn release_unlatches_only_that_param_and_the_last_release_disarms() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 0);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.25);
        print.latch(0, StepParam::Duration, 3.0);
        print.publish_engine_override(&state);
        assert_eq!(
            state.step_print_override.values_for_track(0),
            (Some(0.25), Some(3.0), None),
            "both held params reach the engine override"
        );
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![0]);

        // Velocity's mouse-up: only duration keeps printing (and only
        // duration stays in the engine override).
        assert!(!print.unlatch(StepParam::Velocity), "duration is still held");
        print.publish_engine_override(&state);
        assert_eq!(
            state.step_print_override.values_for_track(0),
            (None, Some(3.0), None)
        );
        let velocity_before = state.pattern.step_data[0].get(1, StepParam::Velocity);
        set_playhead(&state, 1);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![1]);
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Duration), 3.0);
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Velocity),
            velocity_before,
            "a released param must stop printing immediately"
        );

        // The last held param's release ends print mode entirely.
        assert!(print.unlatch(StepParam::Duration));
        assert!(!print.armed());
        print.publish_engine_override(&state);
        assert_eq!(state.step_print_override.values_for_track(0), (None, None, None));
    }

    #[test]
    fn picker_readouts_track_the_latch_while_armed_and_the_cursor_after() {
        let mut rt = Runtime::new();
        rt.register_reactive("SEQ", Vec::new(), true);
        rt.eval_str("(def cursor-step 2)")
            .expect("seed the lisp cursor-step global");

        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.25);
        assert!(
            sync_print_display_fields(&mut rt, &print)
                || matches!(
                    rt.reactive_field_value("SEQ", "fx-step-value-velocity"),
                    Some(Value::Number(value)) if *value == 0.25
                ),
            "armed latch must land in the picker binding"
        );
        assert!(
            matches!(
                rt.reactive_field_value("SEQ", "fx-step-value-velocity"),
                Some(Value::Number(value)) if *value == 0.25
            ),
            "while armed the velocity picker must read the printed value"
        );
        // A sweep update follows the finger, not the cursor step.
        print.latch(0, StepParam::Velocity, 0.75);
        sync_print_display_fields(&mut rt, &print);
        assert!(matches!(
            rt.reactive_field_value("SEQ", "fx-step-value-velocity"),
            Some(Value::Number(value)) if *value == 0.75
        ));

        // Disarm hands the readouts back to the cursor step's stored values.
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.5);
        let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
        restore_cursor_display_fields(&mut rt, &state, 0, &selected_steps);
        assert!(
            matches!(
                rt.reactive_field_value("SEQ", "fx-step-value-velocity"),
                Some(Value::Number(value)) if *value == 0.5
            ),
            "after disarm the picker must show the cursor step's value again"
        );
    }

    #[test]
    fn touching_a_param_on_another_track_restarts_the_latch_there() {
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        print.latch(1, StepParam::Duration, 2.0);
        let mut hit = RollHitRecorded {
            track: 1,
            step: 0,
            delay: 0.0,
            transpose: 0.0,
            velocity: 1.0,
            duration_steps: 0.25,
            beat: 0.0,
        };
        print.override_roll_hit(&mut hit);
        assert_eq!(hit.duration_steps, 2.0);
        // The old track's velocity latch did not carry over.
        assert_eq!(hit.velocity, 1.0);
    }
}
