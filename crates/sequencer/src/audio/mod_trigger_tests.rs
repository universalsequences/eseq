use super::*;
use crate::effects::gatepitch as gp;
use crate::sequencer::{ModInputRole, ModInputRoles, ModSteps};
use std::os::raw::{c_int, c_void};

#[test]
fn value_role_mappings_cover_their_documented_ranges() {
    assert_eq!(mod_duration_steps(0.0), 1.0 / 16.0);
    assert_eq!(mod_duration_steps(0.5), 1.0);
    assert_eq!(mod_duration_steps(1.0), 16.0);
    assert_eq!(mod_note_transpose(0.0), -24.0);
    assert_eq!(mod_note_transpose(0.5), 0.0);
    assert_eq!(mod_note_transpose(0.5 + 7.0 / 48.0), 7.0);
    assert_eq!(mod_note_transpose(1.0), 24.0);
}

#[test]
fn mod_input_roles_pack_round_trips() {
    let roles = ModInputRoles {
        roles: [ModInputRole::Note, ModInputRole::Trigger, ModInputRole::Modulator, ModInputRole::Duration],
        steps: ModSteps::Latch,
    };
    assert_eq!(ModInputRoles::unpack(roles.pack()), roles);
    assert_eq!(ModInputRoles::default().pack(), 0);
    assert!(roles.has_trigger());
    assert_eq!(roles.input_for(ModInputRole::Duration), Some(3));
    assert_eq!(roles.input_for(ModInputRole::Velocity), None);
}

/// Test source for a track's ext mod inputs: output 0 is a gate that rises at
/// `fire_frame` of the node's `fire_call`th block and holds for `hold`
/// frames; output 1 is the constant `level`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PulseState {
    calls: f32,
    fire_call: f32,
    fire_frame: f32,
    hold: f32,
    level: f32,
}

unsafe extern "C" fn pulse_init(state: *mut c_void, _sr: c_int, _bs: c_int, initial: *const c_void) {
    if !initial.is_null() {
        *(state as *mut PulseState) = *(initial as *const PulseState);
    }
}

unsafe extern "C" fn pulse_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let state = &mut *(state as *mut PulseState);
    let gate = std::slice::from_raw_parts_mut(*out.add(0), nframes as usize);
    let level = std::slice::from_raw_parts_mut(*out.add(1), nframes as usize);
    let firing = state.calls == state.fire_call;
    for (frame, sample) in gate.iter_mut().enumerate() {
        let frame = frame as f32;
        *sample = if firing && frame >= state.fire_frame && frame < state.fire_frame + state.hold {
            1.0
        } else {
            0.0
        };
    }
    level.fill(state.level);
    state.calls += 1.0;
}

struct GraphGuard(engine::HeadlessEngine);
impl Drop for GraphGuard {
    fn drop(&mut self) {
        unsafe { self.0.destroy(); }
    }
}

struct Rig {
    guard: GraphGuard,
    _app: crate::app::App,
    data: Box<AudioCallbackData>,
    gatepitch: i32,
}

impl Rig {
    /// A custom track whose ext1 carries a single pulse in the graph's third
    /// block and whose ext2 holds `level`.
    fn new(roles: ModInputRoles, level: f32) -> Self {
        let engine = engine::init_headless_engine(48_000, 2).unwrap();
        let guard = GraphGuard(engine);
        let engine = &guard.0;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = crate::app::App::new(
            engine.state.clone(), engine.lg_ptr, engine.sample_rate,
            engine.buses.clone(), engine.master_recorder.clone(), tx,
        );
        let manifest = crate::lisp_host::DGenManifest {
            effect_latency_samples: None, tail_sleep: None, dylib_path: Default::default(), asset_base: None,
            version: 1, process_abi: String::new(), total_memory_slots: 1,
            params: vec![], groups: vec![], envelopes: vec![], inputs: vec![],
            modulators: vec![], mod_outputs: vec![], amp_output_channel: None, probes: vec![], mod_destinations: vec![],
            n_inputs: 4, n_outputs: 1, tensors: vec![], tensor_init_data: vec![],
            voice_cell_id: None,
            binding_shapes: Default::default(),
        };
        let lib = crate::lisp_host::test_loaded_dgen_lib();
        let engine_id = app.editor.engine_registry.upsert(crate::app::EngineDescriptor {
            name: "mod-trigger".into(), source: "mod-trigger.lisp".into(),
            manifest: manifest.clone(), lib_index: 0, shared_runtime: true,
        });
        app.editor.instrument_libs.push(crate::lisp_host::test_loaded_dgen_lib());
        app.graph_controller().add_custom_track(
            "mod-trigger", engine_id, &manifest, &lib, CustomInstrumentRunMode::Instrument,
        ).unwrap();
        let state = &engine.state;
        state.pattern.track_params[0].set_max_polyphony(1);
        if !state.pattern.track_params[0].is_gate_on() {
            state.pattern.track_params[0].toggle_gate();
        }
        state.pattern.track_params[0].set_mod_roles(roles);
        let gatepitch = app.graph.engine_node_ids[engine_id].as_ref().unwrap().gatepitch_ids[0];
        let clips = app.graph.track_node_ids[0].mod_in_clip_ids;
        let pulse = PulseState { calls: 0.0, fire_call: 2.0, fire_frame: 100.0, hold: 40.0, level };
        let name = std::ffi::CString::new("mod-trigger-pulse").unwrap();
        unsafe {
            assert!(add_node_to_watchlist(engine.lg_ptr.0, gatepitch));
            let pulse_id = add_node(
                engine.lg_ptr.0,
                NodeVTable {
                    process: Some(pulse_process),
                    init: Some(pulse_init),
                    reset: None,
                    migrate: None,
                    begin_event_slice: None,
                    schedule_event: None,
                },
                std::mem::size_of::<PulseState>(),
                name.as_ptr(),
                0,
                2,
                (&pulse as *const PulseState).cast(),
                std::mem::size_of::<PulseState>(),
            );
            assert!(pulse_id > 0);
            graph_connect(engine.lg_ptr.0, pulse_id, 0, clips[0], 0);
            graph_connect(engine.lg_ptr.0, pulse_id, 1, clips[1], 0);
        }
        state.transport.playing.store(true, Ordering::Relaxed);
        let data = new_audio_callback_data(
            engine.lg_ptr.0, state.clone(), 48_000, 2, 512,
            engine.master_recorder.clone(), rx, engine.buses.bus_effect_runtime.clone(),
            Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
        );
        Self { guard, _app: app, data, gatepitch }
    }

    fn render(&mut self, blocks: usize) {
        let mut output = vec![0.0; 1024];
        for _ in 0..blocks {
            audio_callback(&mut self.data, &mut output);
        }
    }

    /// The voice's gatepitch state (watched snapshots publish every few blocks).
    fn voice(&self) -> Vec<f32> {
        let mut slots = vec![0.0_f32; gp::GATEPITCH_STATE_SIZE];
        let mut size = 0;
        unsafe {
            assert!(get_node_state_into(self.guard.0.lg_ptr.0, self.gatepitch,
                slots.as_mut_ptr().cast(), std::mem::size_of_val(slots.as_slice()), &mut size));
        }
        slots
    }
}

fn trigger_and_velocity() -> ModInputRoles {
    let mut roles = ModInputRoles::default();
    roles.roles[0] = ModInputRole::Trigger;
    roles.roles[1] = ModInputRole::Velocity;
    roles
}

#[test]
fn cable_trigger_fires_one_block_later_at_its_frame_with_held_velocity() {
    let mut rig = Rig::new(trigger_and_velocity(), 0.3);
    // The pulse rises in the graph's third block: nothing is pending before.
    rig.render(2);
    assert!(rig.data.mod_triggers.pending.is_empty());
    rig.render(1);
    let pending: Vec<_> = rig.data.mod_triggers.pending.iter().copied().collect();
    assert_eq!(pending.len(), 1, "one rising edge, one trigger");
    let (frame, event) = pending[0];
    assert_eq!(frame, 100, "the next block fires at the edge's own frame");
    assert_eq!(event.track_idx, 0);
    assert!((event.velocity.unwrap() - 0.3).abs() < 1e-6, "velocity is sampled at the edge");
    assert_eq!(event.duration, None);
    assert_eq!(event.note, None);

    // Nothing latched yet: the track's defaults play, with the cable velocity.
    rig.render(5);
    assert!(rig.data.mod_triggers.pending.is_empty());
    let voice = rig.voice();
    assert_eq!(voice[gp::PARAM_GATE as usize], 1.0);
    assert!((voice[gp::PARAM_VELOCITY as usize] - 0.3).abs() < 1e-6);
    let c4_hz = 440.0 * 2f32.powf(-9.0 / 12.0);
    assert!((voice[gp::PARAM_PITCH as usize] - c4_hz).abs() < 0.01, "defaults play the root");
}

#[test]
fn cable_trigger_replays_the_latched_step_with_its_note_override() {
    let mut roles = trigger_and_velocity();
    roles.roles[1] = ModInputRole::Note;
    roles.steps = ModSteps::Latch;
    let mut rig = Rig::new(roles, 0.5 + 7.0 / 48.0);
    rig.render(1);

    // A step at transpose 3 with velocity 0.6 only latches on a latch-mode
    // track; the cable then plays it a fifth up.
    let BlockEventKind::Scheduled(step) = crate::audio::tests::test_block_trigger_with(0, 0, 3.0, 0.6).kind else {
        unreachable!();
    };
    assert!(!rig.data.mod_triggers.note_scheduled_trigger(&step), "latch-mode steps stay silent");
    rig.render(6);
    let voice = rig.voice();
    assert_eq!(voice[gp::PARAM_GATE as usize], 1.0);
    assert!((voice[gp::PARAM_VELOCITY as usize] - 0.6).abs() < 1e-6, "velocity comes from the step");
    let g4_hz = 440.0 * 2f32.powf((7.0 - 9.0) / 12.0);
    assert!((voice[gp::PARAM_PITCH as usize] - g4_hz).abs() < 0.01,
        "the note input replaces the step's transpose: {}", voice[gp::PARAM_PITCH as usize]);
}

#[test]
fn stopped_transport_detects_edges_without_firing() {
    let mut rig = Rig::new(trigger_and_velocity(), 0.3);
    rig.guard.0.state.transport.playing.store(false, Ordering::Relaxed);
    rig.render(8);
    assert!(rig.data.mod_triggers.pending.is_empty());
    assert_eq!(rig.voice()[gp::PARAM_GATE as usize], 0.0);
}

#[test]
fn modulator_only_tracks_never_arm_their_lanes() {
    let mut rig = Rig::new(ModInputRoles::default(), 0.3);
    rig.render(8);
    assert!(rig.data.mod_triggers.pending.is_empty());
    assert!(crate::instruments::mod_trigger_lanes::LaneBlock::read(0, 0).is_none());
}

#[test]
fn play_mode_steps_sound_and_latch_while_auditions_do_neither() {
    let mut rig = Rig::new(trigger_and_velocity(), 0.3);
    rig.render(1);
    let BlockEventKind::Scheduled(step) = crate::audio::tests::test_block_trigger_with(0, 0, 3.0, 0.6).kind else {
        unreachable!();
    };
    assert!(rig.data.mod_triggers.note_scheduled_trigger(&step), "play-mode steps still sound");
    assert!(rig.data.mod_triggers.latches[0].is_some());

    let BlockEventKind::Scheduled(mut audition) =
        crate::audio::tests::test_block_trigger_with(1, 0, 5.0, 1.0).kind
    else {
        unreachable!();
    };
    let mut preview = (*audition.event).clone();
    preview.audition_generation = 7;
    audition.event = Arc::new(preview);
    rig.data.mod_triggers.clear_latches();
    assert!(rig.data.mod_triggers.note_scheduled_trigger(&audition));
    assert!(rig.data.mod_triggers.latches[0].is_none(), "a step preview is not playback");
}
