/*!
Live parameter printing (beads eseq-jc9 and eseq-prm).

While transport and recording are active, touching an eligible parameter
latches a [`PrintTarget`]. Step targets print onto the triggered steps the
scheduler plays while latched; device targets p-lock every step boundary the
playhead passes, triggered or not. The hold ends on control release;
transport/recording-off or a focused-track change also disarms it. Multiple
held targets print together.

Step targets use the engine-side override so the value is audible as it is
played, and the scheduler logs every step it substitutes
([`StepPrintHeard`](sequencer::sequencer::StepPrintHeard)). Those logged
values — not the latch's position when the UI frame runs — are what land in
live `step_data`, so the recorded pass reproduces exactly what was heard.
Device targets write live p-lock storage, but the scheduler resolves every device p-lock family from its published snapshot (deep-copied
at publish time), so instrument, effect, MIDI-FX, and rack targets are all
coalesced into one copy-on-write scheduler-track publish per tick, and bus
targets into one bus-runtime publish. Base values remain untouched
throughout. Every path rides the open `RecordingHistoryTransaction`,
so one record pass undoes as one "Record take" entry rather than creating
device-edit gesture history.
*/

use super::*;
use sequencer::sequencer::DeviceParamPrintTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrintTarget {
    Step(StepParam),
    Instrument {
        param_idx: usize,
    },
    Effect {
        slot_idx: usize,
        param_idx: usize,
    },
    BusEffect {
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
    },
    MidiFx {
        slot_idx: usize,
        param_idx: usize,
    },
    RackSlotParam {
        slot_idx: usize,
        param: RackSlotParam,
    },
    RackSlotInstrument {
        slot_idx: usize,
        param_idx: usize,
    },
    RackSlotEffect {
        rack_slot_idx: usize,
        effect_slot_idx: usize,
        param_idx: usize,
    },
    RackMacro {
        macro_idx: usize,
    },
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
    /// Device targets only: playhead step reported last tick, so each
    /// boundary prints once and a frame that skipped steps can catch up
    /// across the gap (wrap-aware). Step targets print from the heard log.
    last_step: Option<usize>,
    /// Device targets only: a latch/value change since the last tick
    /// re-reports the step under the playhead without a boundary crossing.
    touch_dirty: bool,
    /// Tracks (bitmask) whose printed values are in live pattern state but
    /// not yet in the scheduler snapshot. Per-track so the flush can use
    /// copy-on-write `publish_scheduler_track` publishes — a full snapshot
    /// capture per printing frame starved the (normal-priority) scheduler
    /// thread on weak machines and made the whole track's timing shaky.
    dirty_unpublished_tracks: u64,
}

/// One frame's print result.
///
/// `heard` lists the step params written from the scheduler's heard log as
/// (track, step, param): each is the value that step actually played with.
///
/// `steps`/`targets` are the device-param prints: every step boundary the
/// playhead passed on `track`, and the held device targets to p-lock onto
/// them. Device p-lock prints land on every passed step (an off-step device
/// p-lock is track-level automation: it applies to all voices at that
/// boundary and holds until the next p-lock or ON trigger).
#[derive(Default)]
pub(crate) struct PrintedSteps {
    pub(crate) track: usize,
    pub(crate) steps: Vec<usize>,
    pub(crate) targets: Vec<(PrintTarget, f32)>,
    pub(crate) heard: Vec<(usize, usize, StepParam)>,
}

/// The step params the *step* panel pickers can print, by their Lisp keyword.
/// Single source of truth for `seq-print-step-param` (natives.rs) and the
/// `print-step-param` / `print-step-param-release` host commands: a param
/// accepted by one and not the other silently froze the picker while
/// recording (the retrig pickers shipped that way).
pub(crate) fn print_step_param_from_keyword(name: &str) -> Option<StepParam> {
    match name {
        "velocity" | "vel" => Some(StepParam::Velocity),
        "duration" | "dur" => Some(StepParam::Duration),
        "transpose" => Some(StepParam::Transpose),
        "pan" => Some(StepParam::Pan),
        "retrig" | "rtrg" => Some(StepParam::Retrig),
        "retrig-rate" | "rate" => Some(StepParam::RetrigRate),
        _ => None,
    }
}

impl StepPrintState {
    /// `track`'s printed values are in live pattern state but not yet in the
    /// scheduler snapshot.
    fn mark_dirty(&mut self, track: usize) {
        self.dirty_unpublished_tracks |= 1u64 << track.min(sequencer::sequencer::MAX_TRACKS - 1);
    }

    pub(crate) fn armed(&self) -> bool {
        !self.values.is_empty()
    }

    /// Whether the latch prints on `track`.
    pub(crate) fn armed_on(&self, track: usize) -> bool {
        self.armed() && self.track == track
    }

    /// The value the latch holds for `target` on `track` (what the host
    /// kinds show while it prints: `param.printing`, the step panel's
    /// pickers, a rack macro, a rack slot's strip).
    pub(crate) fn latched(&self, track: usize, target: PrintTarget) -> Option<f32> {
        if self.track != track {
            return None;
        }
        let held = self.values.iter().find(|(held, _)| *held == target);
        held.map(|(_, value)| *value)
    }

    /// Arm-on-touch: latch a param value for printing. A touch on a
    /// different track than the current latch restarts the latch there.
    pub(crate) fn latch(&mut self, track: usize, target: impl Into<PrintTarget>, value: f32) {
        let target = target.into();
        if self.armed() && self.track != track {
            self.disarm();
        }
        self.track = track;
        match self
            .values
            .iter_mut()
            .find(|(existing, _)| *existing == target)
        {
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
    /// an unrelated step-param hold. Republishes the engine override so the
    /// scheduler stops substituting the released device values.
    pub(crate) fn release_device_param_gesture(&mut self, state: &SequencerState) {
        self.values
            .retain(|(target, _)| matches!(target, PrintTarget::Step(_)));
        if self.values.is_empty() {
            self.disarm();
        }
        self.publish_engine_override(state);
    }

    /// Mirror the latch into the engine-side overrides so the scheduler plays
    /// the latched values on the armed track immediately (the pattern/p-lock
    /// writes land behind the playhead — and every trigger stamps device
    /// params from the snapshot — so without this the print would only be
    /// heard one loop later). Call after every latch change; `print_pass`
    /// clears the engine side on disarm.
    pub(crate) fn publish_engine_override(&self, state: &SequencerState) {
        if !self.armed() {
            state.step_print_override.clear();
            state.device_print_override.clear();
            state.rack_macro_print_override.clear();
            return;
        }
        let step_entries = self
            .values
            .iter()
            .filter_map(|(target, value)| match target {
                PrintTarget::Step(param) => Some((*param, *value)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if step_entries.is_empty() {
            state.step_print_override.clear();
        } else {
            state.step_print_override.set(self.track, &step_entries);
        }
        // Device-param analog: latched instrument/effect values substitute at
        // the scheduler's trigger resolution. An empty entry list disarms.
        let device_entries = self
            .values
            .iter()
            .filter_map(|(target, value)| match target {
                PrintTarget::Instrument { param_idx } => Some((
                    DeviceParamPrintTarget::Instrument {
                        param_idx: *param_idx,
                    },
                    *value,
                )),
                PrintTarget::Effect {
                    slot_idx,
                    param_idx,
                } => Some((
                    DeviceParamPrintTarget::Effect {
                        slot_idx: *slot_idx,
                        param_idx: *param_idx,
                    },
                    *value,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        state.device_print_override.set(self.track, device_entries);
        // Rack-macro analog: the audio thread substitutes latched macro
        // positions when it applies rack macros at a trigger.
        let rack_macro_entries = self
            .values
            .iter()
            .filter_map(|(target, value)| match target {
                PrintTarget::RackMacro { macro_idx } => Some((*macro_idx, *value)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if rack_macro_entries.is_empty() {
            state.rack_macro_print_override.clear();
        } else {
            state
                .rack_macro_print_override
                .set(self.track, &rack_macro_entries);
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
    // The latch replaces the base-value write: the control shows it through
    // its kind field (`param.value`, `rack-macro.value`, a rack slot's
    // `device.*-display`), which follows the latch while it prints.
    true
}

/// One frame of print mode. First, write every step the scheduler logged as
/// played with latched step params, with exactly the values it played (this
/// also drains steps scheduled just before a release). Then disarm when the
/// (recording && playing && same focused track) gate fails, otherwise report
/// every step boundary the playhead has passed since the previous frame
/// (walking the wrap, so a loop boundary keeps printing) for the held device
/// targets. Pure state + pattern mutation; invalidations/publishing live in
/// `tick_step_print`.
pub(crate) fn print_pass(
    state: &SequencerState,
    print: &mut StepPrintState,
    focused_track: usize,
    recording: bool,
) -> PrintedSteps {
    let mut printed = PrintedSteps {
        heard: print_heard_steps(state, print, recording),
        ..PrintedSteps::default()
    };
    if !print.armed() {
        return printed;
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
        state.device_print_override.clear();
        state.rack_macro_print_override.clear();
        return printed;
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
    // Step params never print from the boundary walk: the heard log above
    // carries them with the values the scheduler actually played.
    if boundary_steps.is_empty() {
        return printed;
    }
    let device_targets = print
        .values
        .iter()
        .filter(|(target, _)| !matches!(target, PrintTarget::Step(_)))
        .copied()
        .collect::<Vec<_>>();
    if device_targets.is_empty() {
        return printed;
    }
    printed.track = track;
    printed.steps = boundary_steps;
    printed.targets = device_targets;
    printed
}

/// Write the scheduler's heard log into live pattern state: each entry is a
/// step that played with latched step params, carrying the exact values the
/// scheduler substituted. Printing never creates triggers, so a step whose
/// trigger was removed meanwhile is skipped. Outside play+record the entries
/// are dropped: the record take that would own them has already closed (and
/// before this log existed, a closing gate never wrote either).
fn print_heard_steps(
    state: &SequencerState,
    print: &mut StepPrintState,
    recording: bool,
) -> Vec<(usize, usize, StepParam)> {
    let heard = state.step_print_override.drain_heard();
    if heard.is_empty() || !recording || !state.is_playing() {
        return Vec::new();
    }
    let mut writes = Vec::new();
    for entry in heard {
        if entry.track >= state.pattern.patterns.len()
            || entry.step >= sequencer::sequencer::MAX_STEPS
            || !state.pattern.patterns[entry.track].is_active(entry.step)
        {
            continue;
        }
        for (param, value) in entry.values.iter() {
            state.set_step_param_no_publish(entry.track, entry.step, param, value);
            writes.push((entry.track, entry.step, param));
        }
        print.mark_dirty(entry.track);
    }
    writes
}

/// What one `tick_step_print` frame did, for the reactive tick to act on.
#[derive(Default)]
pub(crate) struct StepPrintTick {
    /// Steps were printed: mark the record take changed + redraw.
    pub(crate) printed: bool,
}

/// Per-frame drive, mirroring `tick_roll_record`: run the print pass, push
/// targeted step invalidations for what landed (no `ui_epoch` bump — see the
/// `set-step-param-history` contract), and republish the scheduler snapshot
/// so the written values persist once the latch (and its engine override)
/// ends. Runs while disarmed too when the heard log has entries, so steps
/// scheduled just before a release still print. The publish defers while a
/// roll hold has unpublished pattern writes; the roll's own release publish
/// carries the printed values along.
pub(crate) fn tick_step_print(app: &mut app::App, shared: &SharedHandles) -> StepPrintTick {
    let mut print = shared.step_print.lock().unwrap();
    if !print.armed()
        && print.dirty_unpublished_tracks == 0
        && !shared.state.step_print_override.has_heard()
    {
        return StepPrintTick::default();
    }
    let recording = shared.recording.load(Ordering::Relaxed);
    let focused_track = shared.current_track.load(Ordering::Relaxed);
    let printed = print_pass(&shared.state, &mut print, focused_track, recording);
    let roll_publish_pending = shared.roll_record.lock().unwrap().has_unpublished_writes();
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
    let mut wrote = !printed.heard.is_empty();
    for &(track, step, param) in &printed.heard {
        shared.ui_invalidations.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Param(param.into()),
        });
        if param == StepParam::Duration {
            shared.ui_invalidations.push(UiInvalidation::Step {
                track,
                step,
                change: StepInvalidation::DurationSpan,
            });
        }
    }
    let mut track_snapshot_dirty = false;
    let mut bus_runtime_dirty = false;
    let mut plock_presence_steps = Vec::new();
    for step in &printed.steps {
        let mut wrote_plock = false;
        for (target, value) in &printed.targets {
            match target {
                PrintTarget::Step(_) => unreachable!("print_pass filters step targets out"),
                PrintTarget::Instrument { param_idx } => {
                    let target_wrote =
                        app.print_instrument_plock(printed.track, *step, *param_idx, *value);
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::Effect {
                    slot_idx,
                    param_idx,
                } => {
                    let target_wrote =
                        app.print_effect_plock(printed.track, *step, *slot_idx, *param_idx, *value);
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    track_snapshot_dirty |= target_wrote;
                }
                PrintTarget::BusEffect {
                    bus_idx,
                    slot_idx,
                    param_idx,
                } => {
                    let target_wrote =
                        app.print_bus_effect_plock(*bus_idx, *step, *slot_idx, *param_idx, *value);
                    wrote |= target_wrote;
                    wrote_plock |= target_wrote;
                    bus_runtime_dirty |= target_wrote;
                }
                PrintTarget::MidiFx {
                    slot_idx,
                    param_idx,
                } => {
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
                PrintTarget::RackSlotInstrument {
                    slot_idx,
                    param_idx,
                } => {
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
                    let target_wrote =
                        app.print_rack_macro_plock(printed.track, *step, *macro_idx, *value);
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
    // Every track-scoped device p-lock family (instrument, effect, MIDI-FX,
    // rack) reaches the scheduler only through its published per-track
    // snapshot — live `SlotPLockData` writes are atomic but the snapshot
    // capture deep-copies them, so an unpublished print is inaudible until
    // the next unrelated publish (or transport restart). Bus p-locks live in
    // the separately published bus runtime. Coalesce each publication once
    // per tick after all writes land; never publish once per target/step.
    if track_snapshot_dirty {
        if roll_publish_pending {
            // Same deferral contract as the step-param publish above: a held
            // roll owns the track snapshot until its release grace window, so
            // publishing here would make its written steps audible early. Mark
            // the track dirty and let a later tick (or the roll's own release
            // publish) carry the printed p-locks along.
            shared.step_print.lock().unwrap().mark_dirty(printed.track);
        } else {
            shared.state.publish_scheduler_track(printed.track);
        }
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
    StepPrintTick { printed: wrote }
}

#[cfg(test)]
mod step_print_tests {
    use super::{print_pass, PrintTarget, StepPrintState};
    use sequencer::sequencer::{default_empty_effect_chain, SequencerState, StepParam};
    use std::sync::atomic::Ordering;

    fn playing_state() -> SequencerState {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.transport.playing.store(true, Ordering::Relaxed);
        state
    }

    fn set_playhead(state: &SequencerState, step: usize) {
        state.transport.track_playheads[0].store(step as u32, Ordering::Relaxed);
    }

    /// Stand-in for the scheduler resolving `step` on `track`: it plays the
    /// engine override as it stands now and logs exactly that.
    fn schedule(state: &SequencerState, track: usize, step: usize) {
        let played = state.step_print_override.values_for_track(track);
        state.step_print_override.record_heard(track, step, played);
    }

    /// Latch + mirror into the engine override, as the touch path does.
    fn touch(state: &SequencerState, print: &mut StepPrintState, param: StepParam, value: f32) {
        print.latch(0, param, value);
        print.publish_engine_override(state);
    }

    #[test]
    fn playback_and_record_alone_change_nothing_until_a_param_is_touched() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
        let mut print = StepPrintState::default();
        schedule(&state, 0, 0);
        let printed = print_pass(&state, &mut print, 0, true);
        assert!(printed.heard.is_empty() && printed.steps.is_empty());
        assert_eq!(
            state.pattern.step_data[0].get(0, StepParam::Velocity),
            before
        );
    }

    #[test]
    fn a_heard_step_prints_only_the_touched_param() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(4);
        set_playhead(&state, 4);
        let duration_before = state.pattern.step_data[0].get(4, StepParam::Duration);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Velocity, 0.25);
        // Touching alone writes nothing: the step under the playhead was
        // already scheduled (and heard) with its stored value.
        assert!(print_pass(&state, &mut print, 0, true).heard.is_empty());
        schedule(&state, 0, 4);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.heard, vec![(0, 4, StepParam::Velocity)]);
        assert_eq!(printed.steps, Vec::<usize>::new(), "no device targets held");
        assert_eq!(state.pattern.step_data[0].get(4, StepParam::Velocity), 0.25);
        assert_eq!(
            state.pattern.step_data[0].get(4, StepParam::Duration),
            duration_before
        );
    }

    /// The reported bug: the picker keeps moving after a step was scheduled.
    /// The recording must keep the value the step PLAYED with, not wherever
    /// the picker sat when the UI frame stamped it.
    #[test]
    fn a_step_records_the_value_it_played_with_not_the_pickers_later_position() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(2);
        state.pattern.patterns[0].toggle_step(3);
        set_playhead(&state, 2);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Transpose, -20.0);
        schedule(&state, 0, 2);
        // Drag continues within step 2 before the UI frame runs.
        touch(&state, &mut print, StepParam::Transpose, -26.0);
        print_pass(&state, &mut print, 0, true);
        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Transpose),
            -20.0,
            "step 2 played at -20 and must record -20"
        );
        // Still on step 2 with a newer value: nothing rewrites it.
        touch(&state, &mut print, StepParam::Transpose, -30.0);
        assert!(print_pass(&state, &mut print, 0, true).heard.is_empty());
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Transpose), -20.0);
        // The next step plays (and records) the value current at ITS
        // schedule time.
        schedule(&state, 0, 3);
        set_playhead(&state, 3);
        print_pass(&state, &mut print, 0, true);
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Transpose), -30.0);
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
        touch(&state, &mut print, StepParam::Transpose, 12.0);
        touch(&state, &mut print, StepParam::Duration, 3.5);
        schedule(&state, 0, step);
        assert_eq!(print_pass(&state, &mut print, 0, true).heard.len(), 2);

        assert_eq!(state.pattern.chord_data[0].get(step, 0), 12.0);
        assert_eq!(state.pattern.chord_data[0].get_duration(step, 0), 3.5);
    }

    #[test]
    fn heard_entries_never_create_triggers() {
        let state = playing_state();
        set_playhead(&state, 5);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Duration, 3.0);
        // The trigger was removed between scheduling and the UI frame.
        schedule(&state, 0, 5);
        assert!(print_pass(&state, &mut print, 0, true).heard.is_empty());
        assert!(!state.pattern.patterns[0].is_active(5));
        assert_eq!(
            state.pattern.step_data[0].get(5, StepParam::Duration),
            StepParam::Duration.default_value()
        );
    }

    #[test]
    fn device_targets_walk_every_passed_boundary_across_the_wrap() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(14);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 13);
        let mut print = StepPrintState::default();
        let target = PrintTarget::Instrument { param_idx: 2 };
        print.latch(0, target, 0.5);
        print.latch(0, StepParam::Duration, 3.0);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![13]);
        assert_eq!(
            printed.targets,
            vec![(target, 0.5)],
            "step targets never ride the boundary walk"
        );
        // Frame gap across the wrap: every passed step, triggered or not
        // (off-step device p-locks are track-level automation).
        set_playhead(&state, 1);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![14, 15, 0, 1]);
        // A still latch on the same step does not re-report it; a moved
        // value reprints the current step.
        assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        print.latch(0, target, 0.75);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![1]);
    }

    #[test]
    fn pause_record_off_and_focus_change_each_disarm_without_stray_writes() {
        for (playing, recording, focused_track) in
            [(false, true, 0), (true, false, 0), (true, true, 1)]
        {
            let state = playing_state();
            state.transport.playing.store(playing, Ordering::Relaxed);
            state.pattern.patterns[0].toggle_step(0);
            set_playhead(&state, 0);
            let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
            let mut print = StepPrintState::default();
            print.latch(0, StepParam::Velocity, 0.1);
            let printed = print_pass(&state, &mut print, focused_track, recording);
            assert!(printed.steps.is_empty() && printed.heard.is_empty());
            assert!(
                !print.armed(),
                "gate ({playing}, {recording}, track {focused_track}) must disarm"
            );
            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Velocity),
                before
            );
            state.transport.playing.store(true, Ordering::Relaxed);
            assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        }
    }

    #[test]
    fn heard_steps_outside_play_and_record_are_dropped() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Velocity, 0.1);
        schedule(&state, 0, 0);
        // Record went off: the take that would own the write has closed.
        assert!(print_pass(&state, &mut print, 0, false).heard.is_empty());
        assert!(!state.step_print_override.has_heard(), "the log is drained");
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Velocity), before);
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
        assert!(
            state.step_print_override.values_for_track(0).is_empty(),
            "instrument printing must not use the step engine override"
        );

        print.release_device_param_gesture(&state);
        assert!(!print.armed());
    }

    #[test]
    fn device_gesture_release_removes_instrument_and_effect_targets_only() {
        let state = playing_state();
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
        print.publish_engine_override(&state);
        let device = state
            .device_print_override
            .values_for_track(0)
            .expect("held device params arm the scheduler-side override");
        assert_eq!(
            device.entries,
            vec![
                (
                    sequencer::sequencer::DeviceParamPrintTarget::Instrument { param_idx: 2 },
                    0.6,
                ),
                (
                    sequencer::sequencer::DeviceParamPrintTarget::Effect {
                        slot_idx: 1,
                        param_idx: 3,
                    },
                    0.7,
                ),
            ],
        );

        print.release_device_param_gesture(&state);

        assert!(
            print.armed(),
            "the independent step-param hold remains armed"
        );
        assert_eq!(
            print.values,
            vec![(PrintTarget::Step(StepParam::Velocity), 0.5)]
        );
        assert!(
            state.device_print_override.values_for_track(0).is_none(),
            "release must disarm the scheduler-side device override",
        );
    }

    /// Retrig / Rate are the roll knobs: a drag while play+record must both
    /// reach the engine override (heard now) and print what was heard
    /// (docs/step-retrig-spec.md).
    #[test]
    fn retrig_and_rate_print_what_the_engine_played() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 0);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Retrig, 3.0);
        touch(&state, &mut print, StepParam::RetrigRate, 12.0);

        let overrides = state.step_print_override.values_for_track(0);
        assert_eq!(
            (
                overrides.get(StepParam::Retrig),
                overrides.get(StepParam::RetrigRate),
            ),
            (Some(3.0), Some(12.0)),
            "the held roll knobs must pre-echo through the scheduler override"
        );

        schedule(&state, 0, 0);
        print_pass(&state, &mut print, 0, true);
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Retrig), 3.0);
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::RetrigRate), 12.0);

        // Rate's mouse-up leaves the retrig count printing on its own.
        assert!(!print.unlatch(StepParam::RetrigRate), "retrig is still held");
        print.publish_engine_override(&state);
        let rate_before = state.pattern.step_data[0].get(1, StepParam::RetrigRate);
        schedule(&state, 0, 1);
        set_playhead(&state, 1);
        print_pass(&state, &mut print, 0, true);
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Retrig), 3.0);
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::RetrigRate),
            rate_before,
            "a released param must stop printing immediately"
        );
    }

    #[test]
    fn release_unlatches_one_param_and_already_heard_steps_still_print() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 0);
        let mut print = StepPrintState::default();
        touch(&state, &mut print, StepParam::Velocity, 0.25);
        touch(&state, &mut print, StepParam::Duration, 3.0);
        let overrides = state.step_print_override.values_for_track(0);
        assert_eq!(
            (
                overrides.get(StepParam::Velocity),
                overrides.get(StepParam::Duration),
                overrides.get(StepParam::Transpose),
            ),
            (Some(0.25), Some(3.0), None),
            "both held params reach the engine override"
        );

        // Velocity's mouse-up: only duration stays in the engine override.
        assert!(!print.unlatch(StepParam::Velocity), "duration is still held");
        print.publish_engine_override(&state);
        let velocity_before = state.pattern.step_data[0].get(1, StepParam::Velocity);
        schedule(&state, 0, 1);
        print_pass(&state, &mut print, 0, true);
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Duration), 3.0);
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Velocity),
            velocity_before,
            "a released param must stop printing immediately"
        );

        // A step scheduled inside the lookahead just before the last release
        // was heard with the latch, so it still prints after disarm.
        state.pattern.step_data[0].set(0, StepParam::Duration, 1.0);
        schedule(&state, 0, 0);
        assert!(print.unlatch(StepParam::Duration));
        assert!(!print.armed());
        print.publish_engine_override(&state);
        assert!(state.step_print_override.values_for_track(0).is_empty());
        print_pass(&state, &mut print, 0, true);
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Duration), 3.0);
    }

    #[test]
    fn touching_a_param_on_another_track_restarts_the_latch_there() {
        let state = SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        print.latch(1, StepParam::Duration, 2.0);
        print.publish_engine_override(&state);
        assert!(state.step_print_override.values_for_track(0).is_empty());
        let overrides = state.step_print_override.values_for_track(1);
        assert_eq!(overrides.get(StepParam::Duration), Some(2.0));
        // The old track's velocity latch did not carry over.
        assert_eq!(overrides.get(StepParam::Velocity), None);
    }

    /// Every picker in the *step* panel must be printable through both the
    /// native and the host command, so the keyword table has to cover the
    /// panel's step params exactly (see `seqv-param-keyword`).
    #[test]
    fn every_step_inspector_param_has_a_print_keyword() {
        for param in [
            StepParam::Transpose,
            StepParam::Velocity,
            StepParam::Duration,
            StepParam::Pan,
            StepParam::Retrig,
            StepParam::RetrigRate,
        ] {
            let keyword = match param {
                StepParam::Velocity => "velocity",
                StepParam::Duration => "duration",
                StepParam::Transpose => "transpose",
                StepParam::Pan => "pan",
                StepParam::Retrig => "retrig",
                StepParam::RetrigRate => "retrig-rate",
                other => panic!("{other:?} is a step panel param but has no keyword"),
            };
            assert_eq!(
                super::print_step_param_from_keyword(keyword),
                Some(param),
                "{keyword} must resolve for printing"
            );
        }
    }
}
