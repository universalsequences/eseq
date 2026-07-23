use super::super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectGraphNodeIds {
    pub effect_node_id: i32,
    pub modulator_node_id: Option<i32>,
    /// Edit batch whose application makes any replaced node unreachable.
    pub replacement_batch_serial: u64,
}

// ── Manifest types ──

#[derive(Clone)]
pub struct DGenManifest {
    pub dylib_path: PathBuf,
    pub version: u32,
    pub process_abi: String,
    pub total_memory_slots: usize,
    pub params: Vec<DGenParam>,
    pub groups: Vec<DGenUiGroup>,
    pub envelopes: Vec<DGenEnvelope>,
    pub inputs: Vec<DGenInput>,
    pub modulators: Vec<DGenModulator>,
    pub mod_outputs: Vec<DGenModOutput>,
    pub mod_destinations: Vec<DGenModDestination>,
    pub n_inputs: usize,
    pub n_outputs: usize,
    pub tensors: Vec<TensorMeta>,
    pub tensor_init_data: Vec<TensorInit>,
    /// Memory cell that holds the voice index (0-5) for voice-aware instruments.
    pub voice_cell_id: Option<usize>,
}

#[derive(Clone)]
pub struct DGenParam {
    pub name: String,
    pub cell_id: usize,
    pub cell_span: usize,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub unit: Option<String>,
    pub hidden: bool,
    pub group: Option<String>,
    pub env: Option<String>,
    pub role: Option<String>,
}

#[derive(Clone)]
pub struct DGenUiGroup {
    pub name: String,
}

#[derive(Clone)]
pub struct DGenEnvelope {
    pub name: String,
    pub group: Option<String>,
    pub roles: DGenEnvelopeRoles,
}

#[derive(Clone, Default)]
pub struct DGenEnvelopeRoles {
    pub attack: Option<String>,
    pub decay: Option<String>,
    pub sustain: Option<String>,
    pub release: Option<String>,
}

#[derive(Clone)]
pub struct TensorInit {
    pub offset: usize,
    pub data: Vec<f32>,
}

#[derive(Clone)]
pub struct TensorMeta {
    pub name: String,
    pub cell_offset: usize,
    pub shape: Vec<usize>,
    pub kind: String,
    pub mutable: bool,
    pub source_file: Option<String>,
    pub source_sample_rate: Option<u32>,
}

#[derive(Clone)]
pub struct DGenInput {
    pub channel: usize,
    pub name: String,
}

#[derive(Clone)]
pub struct DGenModulator {
    pub slot: usize,
    pub input_channel: usize,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DGenSidechainInput {
    pub input_channel: usize,
    pub name: String,
}

#[derive(Clone)]
pub struct DGenModOutput {
    pub slot: usize,
    pub channel: usize,
    pub name: String,
    pub range: String,
}

#[derive(Clone)]
pub struct DGenModDestination {
    pub name: String,
    pub param_cell_id: usize,
    pub active_cell_id: usize,
    pub depth_lanes: Vec<DGenModDepthLane>,
    pub mode: String,
    pub min: f32,
    pub max: f32,
    pub unit: Option<String>,
    pub depth_min: Option<f32>,
    pub depth_max: Option<f32>,
}

#[derive(Clone)]
pub struct DGenModDepthLane {
    pub slot: usize,
    pub depth_cell_id: usize,
}

// ── Loaded dylib handle ──

pub struct LoadedDGenLib {
    pub process_fn: DGenProcessFn,
    _handle: *mut c_void,
}

unsafe impl Send for LoadedDGenLib {}
unsafe impl Sync for LoadedDGenLib {}

#[cfg(test)]
pub(crate) fn test_loaded_dgen_lib() -> LoadedDGenLib {
    unsafe extern "C" fn silent_process(
        _inputs: *const *mut f32,
        outputs: *const *mut f32,
        frame_count: c_int,
        _memory_read: *mut c_void,
        _memory_write: *mut c_void,
        _host_sample_rate: c_float,
    ) {
        if outputs.is_null() || frame_count <= 0 {
            return;
        }
        let output = *outputs;
        if !output.is_null() {
            std::ptr::write_bytes(output, 0, frame_count as usize);
        }
    }

    LoadedDGenLib {
        process_fn: silent_process,
        _handle: std::ptr::null_mut(),
    }
}

// ── Compile result (for async compilation) ──


pub fn parse_manifest(json: &str) -> Result<DGenManifest, String> {
    parse_manifest_with_base(json, &output_dir())
}

pub fn parse_manifest_with_base(json: &str, base_dir: &Path) -> Result<DGenManifest, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse manifest: {e}"))?;

    let dylib_name = v["dylib"].as_str().unwrap_or("effect.dylib");
    let dylib_path = base_dir.join(dylib_name);
    let version = v["version"].as_u64().unwrap_or(0) as u32;
    let process_abi = v["processAbi"].as_str().unwrap_or("").to_string();

    let params = v["params"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|p| DGenParam {
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    cell_id: p["cellId"].as_u64().unwrap_or(0) as usize,
                    cell_span: parse_dgen_param_span(p),
                    default: p["default"].as_f64().unwrap_or(0.0) as f32,
                    min: p["min"].as_f64().unwrap_or(0.0) as f32,
                    max: p["max"].as_f64().unwrap_or(1.0) as f32,
                    unit: p["unit"].as_str().map(|s| s.to_string()),
                    hidden: p["hidden"].as_bool().unwrap_or(false),
                    group: p["group"].as_str().map(|s| s.to_string()),
                    env: p["env"].as_str().map(|s| s.to_string()),
                    role: p["role"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let groups = v["groups"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|group| {
                    Some(DGenUiGroup {
                        name: group["name"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let envelopes = v["envelopes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|env| {
                    let roles = &env["roles"];
                    Some(DGenEnvelope {
                        name: env["name"].as_str()?.to_string(),
                        group: env["group"].as_str().map(|s| s.to_string()),
                        roles: DGenEnvelopeRoles {
                            attack: roles["attack"].as_str().map(|s| s.to_string()),
                            decay: roles["decay"].as_str().map(|s| s.to_string()),
                            sustain: roles["sustain"].as_str().map(|s| s.to_string()),
                            release: roles["release"].as_str().map(|s| s.to_string()),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let inputs: Vec<DGenInput> = v["inputs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|inp| DGenInput {
                    channel: inp["channel"].as_u64().unwrap_or(0) as usize,
                    name: inp["name"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let modulators = v["modulators"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|m| DGenModulator {
                    slot: m["slot"].as_u64().unwrap_or(0) as usize,
                    input_channel: m["inputChannel"].as_u64().unwrap_or(0) as usize,
                    name: m["name"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let mod_outputs = v["modOutputs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|m| DGenModOutput {
                    slot: m["slot"].as_u64().unwrap_or(0) as usize,
                    channel: m["channel"].as_u64().unwrap_or(0) as usize,
                    name: m["name"].as_str().unwrap_or("").to_string(),
                    range: m["range"].as_str().unwrap_or("unipolar").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let mod_destinations = v["modDestinations"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|m| DGenModDestination {
                    name: m["name"].as_str().unwrap_or("").to_string(),
                    param_cell_id: m["paramCellId"].as_u64().unwrap_or(0) as usize,
                    active_cell_id: m["activeCellId"].as_u64().unwrap_or(0) as usize,
                    depth_lanes: m["depthLanes"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|lane| DGenModDepthLane {
                                    slot: lane["slot"].as_u64().unwrap_or(0) as usize,
                                    depth_cell_id: lane["depthCellId"].as_u64().unwrap_or(0)
                                        as usize,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    mode: m["mode"].as_str().unwrap_or("").to_string(),
                    min: m["min"].as_f64().unwrap_or(0.0) as f32,
                    max: m["max"].as_f64().unwrap_or(1.0) as f32,
                    unit: m["unit"].as_str().map(|s| s.to_string()),
                    depth_min: m["depthMin"].as_f64().map(|v| v as f32),
                    depth_max: m["depthMax"].as_f64().map(|v| v as f32),
                })
                .collect()
        })
        .unwrap_or_default();

    let n_inputs = inputs.iter().map(|inp| inp.channel + 1).max().unwrap_or(1);
    let n_outputs = v["outputs"].as_array().map(|a| a.len()).unwrap_or(0).max(1);

    let tensor_init_data = v["tensorInitData"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|t| TensorInit {
                    offset: t["offset"].as_u64().unwrap_or(0) as usize,
                    data: t["data"]
                        .as_array()
                        .map(|d| d.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    let tensors = v["tensors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|t| TensorMeta {
                    name: t["name"].as_str().unwrap_or("").to_string(),
                    cell_offset: t["cellOffset"].as_u64().unwrap_or(0) as usize,
                    shape: t["shape"]
                        .as_array()
                        .map(|shape| {
                            shape
                                .iter()
                                .map(|dim| dim.as_u64().unwrap_or(0) as usize)
                                .collect()
                        })
                        .unwrap_or_default(),
                    kind: t["kind"].as_str().unwrap_or("").to_string(),
                    mutable: t["mutable"].as_bool().unwrap_or(false),
                    source_file: t["sourceFile"].as_str().map(|s| s.to_string()),
                    source_sample_rate: t["sourceSampleRate"]
                        .as_f64()
                        .map(|rate| rate.round().max(1.0) as u32),
                })
                .collect()
        })
        .unwrap_or_default();

    let voice_cell_id = v["voiceCellId"].as_u64().map(|id| id as usize);

    Ok(DGenManifest {
        dylib_path,
        version,
        process_abi,
        total_memory_slots: v["totalMemorySlots"].as_u64().unwrap_or(256) as usize,
        params,
        groups,
        envelopes,
        inputs,
        modulators,
        mod_outputs,
        mod_destinations,
        n_inputs,
        n_outputs,
        tensors,
        tensor_init_data,
        voice_cell_id,
    })
}

pub fn instrument_descriptor_from_manifest(
    name: &str,
    manifest: &DGenManifest,
) -> crate::effects::EffectDescriptor {
    let mut desc = crate::effects::EffectDescriptor::from_lisp_manifest(
        name,
        &manifest.params,
        manifest.n_inputs,
        manifest.n_outputs,
    );
    desc.tensor_params = crate::effects::tensor_param_descriptors_from_manifest(
        &manifest.tensors,
        &manifest.tensor_init_data,
    );
    desc.params
        .extend(crate::voice_modulator::ui_param_descriptors());

    append_dgen_modulator_descriptors(&mut desc, manifest);
    append_dgen_modulation_target_params(&mut desc, manifest);

    desc
}

pub fn effect_has_host_modulation(manifest: &DGenManifest) -> bool {
    !manifest.mod_destinations.is_empty()
}

pub(in crate::lisp_host) fn normalized_dgen_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

pub(in crate::lisp_host) fn is_named_sidechain_input(name: &str) -> bool {
    normalized_dgen_name(name).starts_with("sidechain")
}

pub(in crate::lisp_host) fn sidechain_control_name(name: &str) -> String {
    if is_named_sidechain_input(name) || name.trim().is_empty() {
        "sidechain".to_string()
    } else {
        format!("sidechain {name}")
    }
}

pub fn effect_sidechain_inputs(manifest: &DGenManifest) -> Vec<DGenSidechainInput> {
    let has_host_modulation = effect_has_host_modulation(manifest);
    let mut inputs = Vec::new();

    for input in &manifest.inputs {
        if input.channel < 2 {
            continue;
        }

        let modulator = manifest
            .modulators
            .iter()
            .find(|modulator| modulator.input_channel == input.channel);
        if let Some(modulator) = modulator {
            if !has_host_modulation {
                inputs.push(DGenSidechainInput {
                    input_channel: modulator.input_channel,
                    name: sidechain_control_name(&modulator.name),
                });
            }
            continue;
        }

        if is_named_sidechain_input(&input.name) {
            inputs.push(DGenSidechainInput {
                input_channel: input.channel,
                name: sidechain_control_name(&input.name),
            });
        }
    }

    inputs.sort_by_key(|input| input.input_channel);
    inputs
}

pub fn append_effect_host_modulation_controls(
    desc: &mut crate::effects::EffectDescriptor,
    manifest: &DGenManifest,
) {
    if !effect_has_host_modulation(manifest) {
        return;
    }
    desc.params
        .extend(crate::voice_modulator::effect_param_descriptors());
    desc.instrument_modulators = (1..=crate::voice_modulator::SLOT_COUNT)
        .map(|slot| crate::effects::InstrumentModulatorDescriptor {
            slot,
            label: crate::voice_modulator::modulator_slot_label(slot, ""),
        })
        .collect();
    append_dgen_modulation_target_params(desc, manifest);
}

pub(in crate::lisp_host) fn append_dgen_modulator_descriptors(
    desc: &mut crate::effects::EffectDescriptor,
    manifest: &DGenManifest,
) {
    let mut sorted_modulators = manifest.modulators.clone();
    sorted_modulators.sort_by_key(|m| m.slot);
    desc.instrument_modulators = sorted_modulators
        .iter()
        .map(|m| crate::effects::InstrumentModulatorDescriptor {
            slot: m.slot,
            label: crate::voice_modulator::modulator_slot_label(m.slot, &m.name),
        })
        .collect();
}

pub(in crate::lisp_host) fn append_dgen_modulation_target_params(
    desc: &mut crate::effects::EffectDescriptor,
    manifest: &DGenManifest,
) {
    let param_by_cell: std::collections::HashMap<usize, &DGenParam> =
        manifest.params.iter().map(|p| (p.cell_id, p)).collect();
    for dest in &manifest.mod_destinations {
        let base_param_idx = desc
            .params
            .iter()
            .position(|p| p.node_param_idx == (HEADER_SLOTS + dest.param_cell_id) as u32);
        let active_param_idx = desc.params.len();
        let active_default = param_by_cell
            .get(&dest.active_cell_id)
            .map(|p| p.default)
            .unwrap_or(0.0);
        let active_span = param_by_cell
            .get(&dest.active_cell_id)
            .map(|p| p.cell_span as u32)
            .unwrap_or(1)
            .max(1);
        desc.params.push(crate::effects::ParamDescriptor {
            name: format!("__dgen_mod_active__{}", dest.name),
            min: 0.0,
            max: 1.0,
            default: active_default,
            kind: crate::effects::ParamKind::Boolean,
            scaling: crate::effects::ParamScaling::Linear,
            node_param_idx: (HEADER_SLOTS + dest.active_cell_id) as u32,
            node_param_span: active_span,
            host_control: None,
            ui_metadata: None,
        });
        for lane in &dest.depth_lanes {
            let depth_default = param_by_cell
                .get(&lane.depth_cell_id)
                .map(|p| p.default)
                .unwrap_or(0.0);
            let depth_min = dest.depth_min.unwrap_or_else(|| {
                param_by_cell
                    .get(&lane.depth_cell_id)
                    .map(|p| p.min)
                    .unwrap_or(-1.0)
            });
            let depth_max = dest.depth_max.unwrap_or_else(|| {
                param_by_cell
                    .get(&lane.depth_cell_id)
                    .map(|p| p.max)
                    .unwrap_or(1.0)
            });
            let depth_span = param_by_cell
                .get(&lane.depth_cell_id)
                .map(|p| p.cell_span as u32)
                .unwrap_or(1)
                .max(1);
            let depth_param_idx = desc.params.len();
            desc.params.push(crate::effects::ParamDescriptor {
                name: format!("mod {} slot {} amt", dest.name, lane.slot),
                min: depth_min,
                max: depth_max,
                default: depth_default,
                kind: crate::effects::ParamKind::Continuous {
                    unit: dest.unit.clone(),
                },
                scaling: crate::effects::ParamScaling::Linear,
                node_param_idx: (HEADER_SLOTS + lane.depth_cell_id) as u32,
                node_param_span: depth_span,
                host_control: None,
                ui_metadata: None,
            });
            if let Some(base_param_idx) = base_param_idx {
                desc.instrument_modulation_targets.push(
                    crate::effects::InstrumentModulationTarget {
                        base_param_idx,
                        source_param_idx: None,
                        modulator_slot: lane.slot,
                        depth_param_idx,
                        active_param_idx: Some(active_param_idx),
                        depth_min,
                        depth_max,
                        depth_unit: dest.unit.clone(),
                    },
                );
            }
        }
    }

    for target in &desc.instrument_modulation_targets {
        if let Some(active_param_idx) = target.active_param_idx {
            let active = desc
                .instrument_modulation_targets
                .iter()
                .filter(|candidate| candidate.active_param_idx == Some(active_param_idx))
                .any(|candidate| {
                    desc.params
                        .get(candidate.depth_param_idx)
                        .map(|param| param.default.abs() > f32::EPSILON)
                        .unwrap_or(false)
                });
            if let Some(param) = desc.params.get_mut(active_param_idx) {
                param.default = if active { 1.0 } else { 0.0 };
            }
        }
    }
}

// ── Load dylib ──

pub fn load_dylib(path: &Path) -> Result<LoadedDGenLib, String> {
    let c_path =
        CString::new(path.to_str().ok_or("Invalid dylib path")?).map_err(|e| e.to_string())?;

    unsafe {
        let handle = dlopen(c_path.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            let err = CStr::from_ptr(dlerror()).to_string_lossy().to_string();
            return Err(format!("dlopen failed: {err}"));
        }

        let process_sym = CString::new("process").unwrap();
        let process_ptr = dlsym(handle, process_sym.as_ptr());
        if process_ptr.is_null() {
            let err = CStr::from_ptr(dlerror()).to_string_lossy().to_string();
            return Err(format!("dlsym 'process' failed: {err}"));
        }

        Ok(LoadedDGenLib {
            process_fn: std::mem::transmute(process_ptr),
            _handle: handle,
        })
    }
}

// ── Build initial state message (compact) ──

/// Build a compact init message:
/// [slot_id, total_memory_slots, canary, declared_input_count, enabled,
///  process_fn_chunk0..3, num_entries, idx0, val0, ...]
/// The engine zeroes state; init only needs to set non-zero values.
pub(in crate::lisp_host) fn build_init_message(
    slot_id: usize,
    manifest: &DGenManifest,
    process_fn: Option<DGenProcessFn>,
) -> Vec<f32> {
    // Collect all non-zero index/value pairs
    let mut entries: Vec<(usize, f32)> = Vec::new();

    for param in &manifest.params {
        if param.cell_id < manifest.total_memory_slots && param.default != 0.0 {
            for lane in 0..param.cell_span {
                let idx = param.cell_id + lane;
                if idx < manifest.total_memory_slots {
                    entries.push((idx, param.default));
                }
            }
        }
    }

    for tensor in &manifest.tensor_init_data {
        for (i, &val) in tensor.data.iter().enumerate() {
            let idx = tensor.offset + i;
            if idx < manifest.total_memory_slots && val != 0.0 {
                entries.push((idx, val));
            }
        }
    }

    // Header (10) + pairs (2 * N)
    let mut msg = Vec::with_capacity(10 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    let process_fn_chunks = process_fn
        .map(process_fn_pointer_chunks)
        .unwrap_or([0.0; DGEN_PROCESS_FN_CHUNKS]);
    msg.extend(process_fn_chunks);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Add effect to track's audio chain ──
