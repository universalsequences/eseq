use std::ffi::CString;
use std::sync::{Arc, Mutex};

use cpal::Stream;

use crate::audio;
use crate::audiograph::{self, LiveGraphPtr};
use crate::recorder::MasterRecorder;
use crate::reverb;
use crate::sequencer::{BusId, KeyboardTrigger, SequencerState};
use crate::ui::{AudioBuses, BusGateRuntimeState, BusNodeIds};

pub struct Engine {
    pub state: Arc<SequencerState>,
    pub lg_ptr: LiveGraphPtr,
    pub buses: AudioBuses,
    pub sample_rate: u32,
    pub channels: u16,
    pub master_recorder: Arc<MasterRecorder>,
    pub keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
    pub _stream: Stream,
}

impl Engine {
    /// Clean up audiograph resources. Call after dropping the stream.
    pub unsafe fn destroy(&self) {
        audiograph::clear_os_workgroup();
        audiograph::engine_stop_workers();
        audiograph::destroy_live_graph(self.lg_ptr.0);
    }
}

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
    4
}

pub fn init_engine() -> Result<Engine, Box<dyn std::error::Error>> {
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
    let mix_merge_name = CString::new("mix_merge").unwrap();
    let mix_gate_name = CString::new("mix_gate").unwrap();
    let mix_volume_name = CString::new("mix_volume").unwrap();
    let mix_merge_id = unsafe {
        audiograph::add_node(
            lg,
            crate::stereo_panner::stereo_panner_vtable(),
            crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
            mix_merge_name.as_ptr(),
            2,
            2,
            std::ptr::null(),
            0,
        )
    };
    let mix_volume_id = unsafe {
        audiograph::add_node(
            lg,
            crate::stereo_panner::stereo_panner_vtable(),
            crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
            mix_volume_name.as_ptr(),
            2,
            2,
            std::ptr::null(),
            0,
        )
    };
    let mix_gate_id = unsafe {
        audiograph::add_node(
            lg,
            crate::stereo_panner::stereo_panner_vtable(),
            crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
            mix_gate_name.as_ptr(),
            2,
            2,
            std::ptr::null(),
            0,
        )
    };

    // bus_L / bus_R collect the mix, then the Mix bus FX chain feeds the DAC.
    unsafe {
        audiograph::graph_connect(lg, bus_l_id, 0, mix_merge_id, 0);
        audiograph::graph_connect(lg, bus_r_id, 0, mix_merge_id, 1);
        audiograph::graph_connect(lg, mix_merge_id, 0, mix_gate_id, 0);
        audiograph::graph_connect(lg, mix_merge_id, 1, mix_gate_id, 1);
        audiograph::graph_connect(lg, mix_gate_id, 0, mix_volume_id, 0);
        audiograph::graph_connect(lg, mix_gate_id, 1, mix_volume_id, 1);
        audiograph::graph_connect(lg, mix_volume_id, 0, 0, 0);
        if channels > 1 {
            audiograph::graph_connect(lg, mix_volume_id, 1, 0, 1);
        } else {
            audiograph::graph_connect(lg, mix_volume_id, 1, 0, 0);
        }
    }

    let mut default_bus_nodes = vec![BusNodeIds {
        id: BusId::MIX,
        left_id: bus_l_id,
        right_id: bus_r_id,
        merge_id: mix_merge_id,
        gate_id: mix_gate_id,
        volume_id: mix_volume_id,
    }];
    for (id, label) in [(BusId::DEFAULT_A, "bus_A"), (BusId::DEFAULT_B, "bus_B")] {
        let left_name = CString::new(format!("{label}_L")).unwrap();
        let right_name = CString::new(format!("{label}_R")).unwrap();
        let merge_name = CString::new(format!("{label}_merge")).unwrap();
        let gate_name = CString::new(format!("{label}_gate")).unwrap();
        let volume_name = CString::new(format!("{label}_volume")).unwrap();
        let left_id = unsafe { audiograph::live_add_gain(lg, 1.0, left_name.as_ptr()) };
        let right_id = unsafe { audiograph::live_add_gain(lg, 1.0, right_name.as_ptr()) };
        let merge_id = unsafe {
            audiograph::add_node(
                lg,
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
                merge_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let volume_id = unsafe {
            audiograph::add_node(
                lg,
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
                volume_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let gate_id = unsafe {
            audiograph::add_node(
                lg,
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
                gate_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        unsafe {
            audiograph::graph_connect(lg, left_id, 0, merge_id, 0);
            audiograph::graph_connect(lg, right_id, 0, merge_id, 1);
            audiograph::graph_connect(lg, merge_id, 0, gate_id, 0);
            audiograph::graph_connect(lg, merge_id, 1, gate_id, 1);
            audiograph::graph_connect(lg, gate_id, 0, volume_id, 0);
            audiograph::graph_connect(lg, gate_id, 1, volume_id, 1);
            audiograph::graph_connect(lg, volume_id, 0, bus_l_id, 0);
            audiograph::graph_connect(lg, volume_id, 1, bus_r_id, 0);
        }
        default_bus_nodes.push(BusNodeIds {
            id,
            left_id,
            right_id,
            merge_id,
            gate_id,
            volume_id,
        });
    }
    let bus_gate_runtime = Arc::new(Mutex::new(
        default_bus_nodes
            .iter()
            .map(|nodes| BusGateRuntimeState {
                id: nodes.id,
                gate_id: nodes.gate_id,
                sequence: crate::ui::BusGateSequence::default(),
                effect_slots: crate::ui::BusChannelState::default_effect_slots(),
            })
            .collect(),
    ));
    let bus_gate_playheads = Arc::new(Mutex::new(
        default_bus_nodes
            .iter()
            .map(|nodes| (nodes.id, 0usize))
            .collect(),
    ));

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
    let graph_log = env_flag("TINYSEQ_AUDIOGRAPH_TRACE", false);

    unsafe {
        audiograph::enable_rt_logging(rt_log);
        audiograph::enable_graph_logging(graph_log);
        audiograph::enable_rt_time_constraint(mach_rt);
        audiograph::engine_start_workers(workers);
    }
    eprintln!(
        "audiograph: started {workers} worker(s), Mach RT {}, graph trace {}",
        if mach_rt { "enabled" } else { "disabled" },
        if graph_log { "enabled" } else { "disabled" }
    );

    // Create shared sequencer state (start with 0 tracks)
    let state = Arc::new(SequencerState::new(0, vec![]));
    let master_recorder = Arc::new(MasterRecorder::new(sample_rate, channels));

    // Create channel for keyboard triggers
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
        Arc::clone(&bus_gate_runtime),
        Arc::clone(&bus_gate_playheads),
    )?;

    let lg_ptr = LiveGraphPtr(lg);
    let buses = AudioBuses {
        bus_l_id,
        bus_r_id,
        default_bus_nodes,
        bus_gate_runtime,
        bus_gate_playheads,
        reverb_bus_id,
        reverb_node_id,
    };

    Ok(Engine {
        state,
        lg_ptr,
        buses,
        sample_rate,
        channels,
        master_recorder,
        keyboard_tx,
        _stream: stream,
    })
}
