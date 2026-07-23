use super::super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectChainSuccessor {
    StereoNode { node_id: i32, input_channels: usize },
    MonoPair { left: i32, right: i32 },
}

/// Remove an effect from the chain and reconnect predecessor → successor.
pub unsafe fn remove_effect_from_chain(
    lg: *mut LiveGraph,
    effect_node_id: i32,
    predecessor_id: i32,
    successor_id: i32,
) {
    remove_effect_from_chain_at_successor(
        lg,
        effect_node_id,
        predecessor_id,
        EffectChainSuccessor::StereoNode {
            node_id: successor_id,
            input_channels: 2,
        },
    );
}

pub unsafe fn remove_effect_from_chain_at_successor(
    lg: *mut LiveGraph,
    effect_node_id: i32,
    predecessor_id: i32,
    successor: EffectChainSuccessor,
) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            audiograph::graph_disconnect(lg, predecessor_id, src_port, effect_node_id, dst_port);
            match successor {
                EffectChainSuccessor::StereoNode { node_id, .. } => {
                    audiograph::graph_disconnect(lg, effect_node_id, src_port, node_id, dst_port);
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, node_id, dst_port);
                }
                EffectChainSuccessor::MonoPair { left, right } => {
                    audiograph::graph_disconnect(lg, effect_node_id, src_port, left, 0);
                    audiograph::graph_disconnect(lg, effect_node_id, src_port, right, 0);
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, left, 0);
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, right, 0);
                }
            }
        }
    }
    audiograph::delete_node(lg, effect_node_id);
}

pub unsafe fn remove_effect_modulator(lg: *mut LiveGraph, modulator_node_id: i32) {
    if modulator_node_id > 0 {
        audiograph::delete_node(lg, modulator_node_id);
    }
}

pub(in crate::lisp_host) unsafe fn disconnect_direct_chain(
    lg: *mut LiveGraph,
    predecessor_id: i32,
    successor: EffectChainSuccessor,
) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            match successor {
                EffectChainSuccessor::StereoNode { node_id, .. } => {
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, node_id, dst_port);
                }
                EffectChainSuccessor::MonoPair { left, right } => {
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, left, 0);
                    audiograph::graph_disconnect(lg, predecessor_id, src_port, right, 0);
                }
            }
        }
    }
}

pub(in crate::lisp_host) unsafe fn connect_effect_port(
    lg: *mut LiveGraph,
    src_node: i32,
    src_port: i32,
    dst_node: i32,
    dst_port: i32,
    context: &str,
) -> Result<(), String> {
    if audiograph::graph_connect(lg, src_node, src_port, dst_node, dst_port) {
        Ok(())
    } else {
        Err(format!(
            "{context}: graph_connect({src_node}, {src_port}, {dst_node}, {dst_port}) failed"
        ))
    }
}

pub(in crate::lisp_host) unsafe fn connect_effect_chain(
    lg: *mut LiveGraph,
    predecessor_id: i32,
    predecessor_outputs: usize,
    effect_id: i32,
    effect_inputs: usize,
    effect_outputs: usize,
    successor: EffectChainSuccessor,
) -> Result<(), String> {
    if effect_inputs <= 1 {
        let pred_channels = predecessor_outputs.max(1).min(2);
        for src_port in 0..pred_channels {
            connect_effect_port(
                lg,
                predecessor_id,
                src_port as i32,
                effect_id,
                0,
                "connect effect input",
            )?;
        }
    } else {
        let pred_channels = predecessor_outputs.max(1).min(2);
        for ch in 0..pred_channels.min(effect_inputs).min(2) {
            connect_effect_port(
                lg,
                predecessor_id,
                ch as i32,
                effect_id,
                ch as i32,
                "connect effect input",
            )?;
        }
    }

    match successor {
        EffectChainSuccessor::StereoNode {
            node_id,
            input_channels,
        } => {
            if effect_outputs <= 1 {
                for dst_port in 0..input_channels.max(1).min(2) {
                    connect_effect_port(
                        lg,
                        effect_id,
                        0,
                        node_id,
                        dst_port as i32,
                        "connect effect output",
                    )?;
                }
            } else {
                for ch in 0..input_channels.max(1).min(2).min(effect_outputs) {
                    connect_effect_port(
                        lg,
                        effect_id,
                        ch as i32,
                        node_id,
                        ch as i32,
                        "connect effect output",
                    )?;
                }
            }
        }
        EffectChainSuccessor::MonoPair { left, right } => {
            connect_effect_port(lg, effect_id, 0, left, 0, "connect effect left output")?;
            connect_effect_port(
                lg,
                effect_id,
                if effect_outputs > 1 { 1 } else { 0 },
                right,
                0,
                "connect effect right output",
            )?;
        }
    }

    Ok(())
}

/// Add a DGenLisp effect between predecessor and successor nodes.
/// slot_id = track_idx * MAX_CUSTOM_FX + offset.
pub unsafe fn add_effect_to_chain_at(
    lg: *mut LiveGraph,
    slot_id: usize,
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    predecessor_id: i32,
    predecessor_outputs: usize,
    successor_id: i32,
    successor_inputs: usize,
    existing_effect: Option<i32>,
    existing_modulator: Option<i32>,
    ext_mod_input_nodes: Option<&[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]>,
) -> Result<EffectGraphNodeIds, String> {
    add_effect_to_chain_at_successor(
        lg,
        slot_id,
        manifest,
        lib,
        predecessor_id,
        predecessor_outputs,
        EffectChainSuccessor::StereoNode {
            node_id: successor_id,
            input_channels: successor_inputs,
        },
        existing_effect,
        existing_modulator,
        ext_mod_input_nodes,
    )
}

/// Add a DGenLisp effect between a predecessor and an explicitly shaped
/// successor. Rack slots terminate at independent mono voice-sum nodes, while
/// ordinary track and bus chains terminate at a stereo node.
pub unsafe fn add_effect_to_chain_at_successor(
    lg: *mut LiveGraph,
    slot_id: usize,
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    predecessor_id: i32,
    predecessor_outputs: usize,
    successor: EffectChainSuccessor,
    existing_effect: Option<i32>,
    existing_modulator: Option<i32>,
    ext_mod_input_nodes: Option<&[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]>,
) -> Result<EffectGraphNodeIds, String> {
    // Full state allocation (header + distinct read/write buffers), zeroed by the engine
    let state_size =
        dgen_total_state_slots(manifest.total_memory_slots) * std::mem::size_of::<f32>();

    // Compact init message: only header + non-zero index/value pairs
    let init_msg = build_init_message(slot_id, manifest, Some(lib.process_fn));
    let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();

    let name = CString::new(format!("dgenlisp_fx_{}", slot_id)).unwrap();

    let node_id = audiograph::add_node(
        lg,
        dgenlisp_vtable(),
        state_size,
        name.as_ptr(),
        manifest.n_inputs as c_int,
        manifest.n_outputs as c_int,
        init_msg.as_ptr() as *const c_void,
        init_msg_size,
    );

    if node_id < 0 {
        return Err("Failed to add DGenLisp node to graph".to_string());
    }

    let modulator_node_id = if effect_has_host_modulation(manifest) {
        let mod_name = CString::new(format!("dgenlisp_fx_{}_mod", slot_id)).unwrap();
        let mod_id = audiograph::add_node(
            lg,
            crate::voice_modulator::effect_modulator_vtable(),
            crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
            mod_name.as_ptr(),
            crate::voice_modulator::INPUT_COUNT as c_int,
            crate::voice_modulator::NUM_OUTPUTS as c_int,
            std::ptr::null(),
            0,
        );
        if mod_id < 0 {
            audiograph::delete_node(lg, node_id);
            return Err("Failed to add DGenLisp effect modulator node to graph".to_string());
        }
        Some(mod_id)
    } else {
        None
    };

    // Each node owns its immutable process-function identity. The old and new
    // nodes may therefore coexist safely while this edit batch crosses the
    // audio-thread boundary.
    audiograph::begin_graph_edit_batch(lg);
    let replacement_batch_serial = audiograph::graph_edit_current_batch_serial(lg);
    let connect_result = connect_effect_chain(
        lg,
        predecessor_id,
        predecessor_outputs,
        node_id,
        manifest.n_inputs,
        manifest.n_outputs,
        successor,
    );
    if let Err(error) = connect_result {
        audiograph::delete_node(lg, node_id);
        if let Some(mod_id) = modulator_node_id {
            audiograph::delete_node(lg, mod_id);
        }
        audiograph::end_graph_edit_batch(lg);
        return Err(error);
    }

    let mod_connect_result = (|| {
        if let Some(mod_id) = modulator_node_id {
            if let Some(ext_nodes) = ext_mod_input_nodes {
                for (input, &ext_node) in ext_nodes.iter().enumerate() {
                    connect_effect_port(
                        lg,
                        ext_node,
                        0,
                        mod_id,
                        (4 + input) as i32,
                        "connect effect modulator ext input",
                    )?;
                }
            }
            for modulator in &manifest.modulators {
                if !(1..=crate::voice_modulator::SLOT_COUNT).contains(&modulator.slot) {
                    continue;
                }
                connect_effect_port(
                    lg,
                    mod_id,
                    (modulator.slot - 1) as i32,
                    node_id,
                    modulator.input_channel as i32,
                    "connect effect modulator output",
                )?;
            }
        }
        Ok(())
    })();
    if let Err(error) = mod_connect_result {
        audiograph::delete_node(lg, node_id);
        if let Some(mod_id) = modulator_node_id {
            audiograph::delete_node(lg, mod_id);
        }
        audiograph::end_graph_edit_batch(lg);
        return Err(error);
    }

    if let Some(old_id) = existing_effect {
        remove_effect_from_chain_at_successor(lg, old_id, predecessor_id, successor);
    } else {
        disconnect_direct_chain(lg, predecessor_id, successor);
    }
    if let Some(old_mod_id) = existing_modulator {
        remove_effect_modulator(lg, old_mod_id);
    }
    audiograph::end_graph_edit_batch(lg);

    Ok(EffectGraphNodeIds {
        effect_node_id: node_id,
        modulator_node_id,
        replacement_batch_serial,
    })
}

// ── Full interactive editor-compile-load flow ──
