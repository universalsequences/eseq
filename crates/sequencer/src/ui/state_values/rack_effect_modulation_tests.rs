use super::*;
use sequencer::audiograph as ag;
use sequencer::instruments::voice_modulator as vm;

/// A deterministic source for the production watchlist reader. The state tail
/// has the real modulator ABI, but no oscillator/timing dependence: this test
/// covers telemetry routing, not the separately tested modulation DSP.
struct TelemetryGraph(ag::LiveGraphPtr);

impl TelemetryGraph {
    fn new() -> Self {
        let raw = unsafe { ag::create_live_graph(16, 64, c"rack modulation telemetry".as_ptr(), 2) };
        assert!(!raw.is_null());
        Self(ag::LiveGraphPtr(raw))
    }

    fn node(&self, value: f32, phase: f32) -> i32 {
        unsafe extern "C" fn init(state: *mut std::ffi::c_void, _: i32, _: i32,
                                  initial: *const std::ffi::c_void) {
            unsafe { std::ptr::copy_nonoverlapping(initial.cast::<f32>(), state.cast::<f32>(), vm::STATE_SIZE) };
        }
        let mut state = vec![0.0_f32; vm::STATE_SIZE];
        state[vm::STATE_DISPLAY_SLOT_VALUE] = value;
        state[vm::STATE_DISPLAY_SLOT_PHASE] = phase;
        let bytes = state.len() * std::mem::size_of::<f32>();
        let id = unsafe { ag::add_node(self.0.0, ag::NodeVTable {
            process: None, init: Some(init), reset: None, migrate: None,
            begin_event_slice: None, schedule_event: None,
        }, bytes, c"telemetry source".as_ptr(), 0, 0, state.as_ptr().cast(), bytes) };
        assert!(id > 0);
        id
    }

    fn render(&self) {
        let mut output = [0.0_f32; 128];
        unsafe { ag::process_next_block(self.0.0, output.as_mut_ptr(), 64) };
    }
}

impl Drop for TelemetryGraph {
    fn drop(&mut self) {
        unsafe { ag::destroy_live_graph(self.0.0) };
    }
}

#[test]
fn rack_effect_modulation_reads_live_tails_and_retires_watchers() {
    let graph = TelemetryGraph::new();
    let app = test_app_with_rack_panel();
    let desc = sequencer::effects::EffectDescriptor::builtin_filter();
    let cutoff = desc.params.iter().position(|p| p.name == "cutoff").unwrap();
    let lane = desc.instrument_modulation_targets.iter()
        .find(|lane| lane.base_param_idx == cutoff && lane.modulator_slot == 1).unwrap();
    let effect_nodes = [graph.node(0.0, 0.0), graph.node(0.0, 0.0)];
    let nodes = [graph.node(0.25, 0.125), graph.node(0.75, 0.625)];
    let mut rack = app.state.live_rack_track_snapshot(0).unwrap();
    rack.slots.push(rack.slots[0].clone());
    for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
        let mut effect = sequencer::effects::EffectSlotSnapshot::new_default_with_modulator(
            &desc, effect_nodes[slot_idx] as u32, nodes[slot_idx] as u32);
        effect.defaults[cutoff] = 1_000.0;
        effect.defaults[lane.depth_param_idx] = 2.0;
        if let Some(idx) = lane.active_param_idx { effect.defaults[idx] = 1.0; }
        if let Some(idx) = lane.source_param_idx { effect.defaults[idx] = 1.0; }
        effect.set_plock(3, cutoff, 2_000.0);
        slot.effect_descriptors[0] = desc.clone();
        slot.effect_slots[0] = effect;
    }
    app.state.set_rack_track_for_all_pattern_snapshots(0, rack);
    graph.render();
    let mut watched = HashSet::new();
    let sample = |live, selected_step, watched: &mut HashSet<i32>| {
        read_mod_display_values(graph.0, &app, &app.state, Some(0), selected_step, live, watched)
    };
    sample(true, None, &mut watched);
    assert_eq!(watched, HashSet::from(nodes));
    // Watchlist snapshots are throttled, not published on every callback.
    for _ in 0..8 { graph.render(); }
    let live = sample(true, None, &mut watched);
    assert_eq!(live.effects.len(), 2);
    for (slot_idx, node) in effect_nodes.iter().enumerate() {
        let effect = live.effects.iter().find(|effect| effect.node_id == *node).unwrap();
        let value = effect.values.iter().find(|value| value.param_idx == cutoff).unwrap();
        let expected = 1_000.0 * 2.0_f64.powf(if slot_idx == 0 { 0.5 } else { 1.5 });
        assert!((value.value - expected).abs() < expected * 0.01, "{value:?}");
        assert!(value.offset > 0.0 && value.scale > 1.0);
        assert_eq!(effect.slot_phases[0], if slot_idx == 0 { 0.125 } else { 0.625 });
    }
    // The same resolution as the knob: selection and playback p-locks shift
    // the base, but not the sampled modulation factor.
    let selected = sample(true, Some(3), &mut watched);
    app.state.transport.playing.store(true, Ordering::Relaxed);
    app.state.transport.track_playheads[0].store(3, Ordering::Relaxed);
    let played = sample(true, None, &mut watched);
    assert_eq!(selected.effects, played.effects);
    for effect in &selected.effects {
        let value = effect.values.iter().find(|value| value.param_idx == cutoff).unwrap();
        assert!((value.value / value.scale - 2_000.0).abs() < 0.01);
    }
    app.state.transport.playing.store(false, Ordering::Relaxed);
    let hidden = sample(false, None, &mut watched);
    assert!(watched.is_empty());
    for effect in &hidden.effects {
        let value = effect.values.iter().find(|value| value.param_idx == cutoff).unwrap();
        assert_eq!((value.value, value.offset, value.scale), (1_000.0, 0.0, 1.0));
        assert_eq!(effect.slot_phases, NO_SLOT_PHASES);
    }
    // Clearing a route also settles back to base while the panel stays open.
    app.state.update_rack_slot_in_current_pattern(0, 0, |slot| {
        slot.effect_slots[0].defaults[lane.depth_param_idx] = 0.0;
    });
    let unassigned = sample(true, None, &mut watched);
    assert!(!watched.contains(&nodes[0]));
    let value = unassigned.effects.iter().find(|effect| effect.node_id == effect_nodes[0]).unwrap()
        .values.iter().find(|value| value.param_idx == cutoff).unwrap();
    assert_eq!((value.value, value.offset, value.scale), (1_000.0, 0.0, 1.0));
}
