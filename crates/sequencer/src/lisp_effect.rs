use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use eseqlisp::frame as eseq_frame;
use eseqlisp::tui as eseq_tui;
use eseqlisp::vm::Value as EValue;
use eseqlisp::{BufferMode, CompileKind, Editor, EditorConfig, HostCommand, HostEvent, Runtime};
use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::audiograph::{self, LiveGraph, NodeVTable};
use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::neural::{
    NeuralMaxPolySelection, ParamNodeId, ProjectEffectParamOverride, ProjectNeuralNetwork,
    ProjectNeuron, ProjectParamOverride, NUM_NEURONS,
};
use crate::scheduled_event::{
    ScheduledEffectParam, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
};
use crate::sequencer::{CustomInstrumentRunMode, StepParam, StepSnapshot, Timebase};

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct MidiFxDescriptorCache {
    source: String,
    descriptors: Vec<EffectDescriptor>,
}

static MIDI_FX_DESCRIPTOR_CACHE: OnceLock<Mutex<Option<MidiFxDescriptorCache>>> = OnceLock::new();

fn read_eseqlisp_init_source() -> String {
    eseqlisp_init_candidates()
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    crate::paths::eseqlisp_init_candidates()
}

// ── dlopen FFI (macOS) ──

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

type DGenProcessFn = unsafe extern "C" fn(
    inputs: *const *mut f32,
    outputs: *const *mut f32,
    frame_count: c_int,
    memory_read: *mut c_void,
    memory_write: *mut c_void,
    host_sample_rate: c_float,
);

// ── Global process function registry ──
// Each track can have up to MAX_CUSTOM_FX custom effects.
// The process fn pointer is stored here, indexed by slot_id = track * MAX_CUSTOM_FX + offset.

use crate::sequencer::MAX_TRACKS;
pub const MAX_CUSTOM_FX: usize = 8;
pub const MAX_MIDI_FX_SLOTS: usize = 4;
pub const MAX_BUS_FX_CHAINS: usize = 64;
const REGISTRY_SIZE: usize = (MAX_TRACKS + MAX_BUS_FX_CHAINS) * MAX_CUSTOM_FX;
static DGEN_PROCESS_FNS: [AtomicUsize; REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; REGISTRY_SIZE]
};

fn set_dgen_process_fn(slot_id: usize, f: DGenProcessFn) {
    set_dgen_process_fn_raw(slot_id, f as usize);
}

fn dgen_process_fn_raw(slot_id: usize) -> usize {
    DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].load(Ordering::Acquire)
}

fn set_dgen_process_fn_raw(slot_id: usize, f: usize) {
    DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].store(f, Ordering::Release);
}

// ── Node state layout ──
// state[0] = slot_id (f32), where slot_id = track_idx * MAX_CUSTOM_FX + offset
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4] = enabled (0 = bypass/silent, 1 = active)
// state[5] = host sample rate
// state[6..6+N] = DGenLisp read buffer
// state[...]     = DGenLisp write buffer (separate to respect `restrict`)

pub const DGEN_ENABLED_PARAM_IDX: usize = 4;
pub const DGEN_HOST_SAMPLE_RATE_IDX: usize = 5;
pub const HEADER_SLOTS: usize = 6;
pub const DGEN_STATE_REDZONE_SLOTS: usize = 256;
const HEADER_CANARY: f32 = f32::from_bits(0x4cd35a1d);

fn ensure_enabled_param(params: &mut Vec<crate::effects::ParamDescriptor>) {
    if params
        .iter()
        .any(|param| param.name.eq_ignore_ascii_case("enabled"))
    {
        return;
    }
    params.push(EffectDescriptor::enabled_param(params.len() as u32, 1.0));
}

pub fn dgen_buffer_span_slots(total_memory_slots: usize) -> usize {
    total_memory_slots + DGEN_STATE_REDZONE_SLOTS
}

pub fn dgen_total_state_slots(total_memory_slots: usize) -> usize {
    HEADER_SLOTS + dgen_buffer_span_slots(total_memory_slots) * 2
}

unsafe fn dgen_read_buffer_ptr(state: *mut f32) -> *mut f32 {
    state.add(HEADER_SLOTS)
}

unsafe fn dgen_write_buffer_ptr(state: *mut f32, total_memory_slots: usize) -> *mut f32 {
    state.add(HEADER_SLOTS + dgen_buffer_span_slots(total_memory_slots))
}

unsafe fn dgen_host_sample_rate(state: *mut f32) -> f32 {
    let sample_rate = *state.add(DGEN_HOST_SAMPLE_RATE_IDX);
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        44_100.0
    }
}

unsafe extern "C" fn dgenlisp_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let s = state as *mut f32;
    let slot_id = (*s) as usize;
    if (*s.add(2)).to_bits() != HEADER_CANARY.to_bits() {
        return;
    }
    if *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5 {
        if inp.is_null() || out.is_null() {
            return;
        }
        let nf = nframes as usize;
        let input_count = (*s.add(3)).max(1.0) as usize;
        for ch in 0..input_count.min(2) {
            let in_ch = *inp.add(ch);
            let out_ch = *out.add(ch);
            if !in_ch.is_null() && !out_ch.is_null() {
                std::ptr::copy_nonoverlapping(in_ch as *const f32, out_ch, nf);
            }
        }
        return;
    }
    let fn_ptr = DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let _total_memory_slots = *s.add(1) as usize;
        let memory_read = dgen_read_buffer_ptr(s) as *mut c_void;
        let memory_write = dgen_write_buffer_ptr(s, _total_memory_slots) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        process_fn(
            inp,
            out,
            nframes,
            memory_read,
            memory_write,
            dgen_host_sample_rate(s),
        );
    } else {
        // Passthrough: copy input to output
        let nf = nframes as usize;
        let in0 = *inp.add(0);
        let out0 = *out.add(0);
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
    }
}

/// Initial state message format (compact, not full-size):
///   [0] = slot_id
///   [1] = total_memory_slots
///   [2] = canary
///   [3] = declared input count
///   [4] = enabled
///   [5] = num_entries (N)
///   [6..6+2N] = pairs of (index, value)
unsafe extern "C" fn dgenlisp_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    if initial_state.is_null() {
        return;
    }
    let src = initial_state as *const f32;
    let dst = state as *mut f32;

    // Copy header
    *dst = *src; // slot_id
    *dst.add(1) = *src.add(1); // total_memory_slots
    *dst.add(2) = *src.add(2); // canary
    *dst.add(3) = *src.add(3); // declared input count
    *dst.add(DGEN_ENABLED_PARAM_IDX) = *src.add(4); // enabled
    *dst.add(DGEN_HOST_SAMPLE_RATE_IDX) = (sample_rate.max(1)) as f32;

    // Apply sparse index/value pairs into the memory region
    let num_entries = (*src.add(5)) as usize;
    let total_memory_slots = *dst.add(1) as usize;
    let mem = dgen_read_buffer_ptr(dst);
    for i in 0..num_entries {
        let idx = (*src.add(6 + i * 2)) as usize;
        let val = *src.add(6 + i * 2 + 1);
        *mem.add(idx) = val;
    }
    let write_mem = dgen_write_buffer_ptr(dst, total_memory_slots);
    std::ptr::copy_nonoverlapping(mem as *const f32, write_mem, total_memory_slots);
}

/// Queue a bulk write of `data` into a live dgenlisp effect node's state at the
/// given tensor `cell_offset` (from the manifest's `tensors[]`). The write lands
/// in the read-state buffer (`HEADER_SLOTS + cell_offset`) — the same region
/// params are written to and the buffer the DSP reads constant inputs from. The
/// engine applies it on the audio thread at a block boundary and copies the data
/// internally, so `data` may be freed immediately after this returns.
pub unsafe fn queue_tensor_write(
    lg: *mut LiveGraph,
    node_id: i32,
    cell_offset: usize,
    data: &[f32],
) -> bool {
    audiograph::write_node_state(
        lg,
        node_id,
        HEADER_SLOTS + cell_offset,
        data.as_ptr(),
        data.len(),
    )
}

pub unsafe fn queue_dgen_host_sample_rate_update(
    lg: *mut LiveGraph,
    node_id: i32,
    sample_rate: u32,
) -> bool {
    let value = sample_rate.max(1) as f32;
    audiograph::write_node_state(lg, node_id, DGEN_HOST_SAMPLE_RATE_IDX, &value, 1)
}

fn dgenlisp_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectGraphNodeIds {
    pub effect_node_id: i32,
    pub modulator_node_id: Option<i32>,
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

// ── Compile result (for async compilation) ──

pub struct CompileResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
}

#[derive(Clone, Debug)]
pub struct InstrumentRenderOptions {
    pub sample_rate: u32,
    pub block_size: usize,
    pub frames: usize,
    pub midi_note: f32,
    pub velocity: f32,
    pub gate_frames: usize,
    pub voice_index: usize,
    pub param_overrides: Vec<(String, f32)>,
    pub param_events: Vec<InstrumentParamEvent>,
    pub input_overrides: Vec<(usize, f32)>,
}

#[derive(Clone, Debug)]
pub struct InstrumentParamEvent {
    pub frame: usize,
    pub name: String,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct InstrumentRenderReport {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    pub mean_abs: f32,
    pub nonzero_frames: usize,
    pub first_nonzero_frame: Option<usize>,
    pub non_finite_samples: usize,
    pub first_non_finite_frame: Option<usize>,
    pub non_finite_state_slots: usize,
    pub first_non_finite_state_slot: Option<usize>,
    pub first_samples: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct EffectRenderOptions {
    pub sample_rate: u32,
    pub block_size: usize,
    pub frames: usize,
    pub param_overrides: Vec<(String, f32)>,
    pub input_overrides: Vec<(usize, f32)>,
}

#[derive(Clone, Debug)]
pub struct EffectRenderReport {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    pub left_rms: f32,
    pub right_rms: f32,
    pub mean_abs: f32,
    pub diff_rms: f32,
    pub nonzero_frames: usize,
    pub first_nonzero_frame: Option<usize>,
    pub first_samples: Vec<f32>,
}

pub fn compile_and_load(source: &str, sample_rate: u32) -> Result<CompileResult, String> {
    compile_and_load_with_asset_base(source, sample_rate, None)
}

pub fn compile_and_load_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_lisp_with_asset_base(source, sample_rate, asset_base)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult { manifest, lib })
}

// ── Effect library storage ──

const EFFECTS_DIR: &str = "effects";
const INSTRUMENTS_DIR: &str = "instruments";

pub fn save_effect(name: &str, source: &str) -> io::Result<()> {
    let path = effect_source_path(name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn save_effect_ui(name: &str, source: &str) -> io::Result<()> {
    let path = effect_ui_path(name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn list_saved_effects() -> Vec<String> {
    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                collect(&path, root, out);
            } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                let file_stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if matches!(file_stem, "dsp" | "ui") {
                    continue;
                }
                out.push(file_stem.to_string());
            }
        }
    }

    let dir = Path::new(EFFECTS_DIR);
    let mut names = Vec::new();
    collect(dir, dir, &mut names);
    names.sort();
    names.dedup();
    names
}

pub fn load_effect_source(name: &str) -> io::Result<String> {
    let path = effect_source_path(name);
    std::fs::read_to_string(&path)
}

pub fn load_effect_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(effect_ui_path(name))
}

pub fn effect_source_path(name: &str) -> PathBuf {
    let root = Path::new(EFFECTS_DIR);
    if name.ends_with('/') {
        return root.join(name.trim_end_matches('/')).join("dsp.lisp");
    }
    let folder_dsp = root.join(name).join("dsp.lisp");
    if folder_dsp.exists() {
        folder_dsp
    } else {
        root.join(format!("{name}.lisp"))
    }
}

pub fn effect_ui_path(name: &str) -> PathBuf {
    Path::new(EFFECTS_DIR)
        .join(name.trim_end_matches('/'))
        .join("ui.lisp")
}

// ── Editor flow ──

pub fn edit_text(initial: &str) -> io::Result<String> {
    let dir = std::env::temp_dir();
    let path = dir.join("sequencer_lisp_edit.lisp");
    std::fs::write(&path, initial)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("editor exited with status: {status}"),
        ));
    }

    std::fs::read_to_string(&path)
}

// ── Compile ──

fn output_dir() -> PathBuf {
    std::env::temp_dir().join("sequencer_dgenlisp")
}

pub fn compile_lisp(source: &str, sample_rate: u32) -> Result<String, String> {
    compile_lisp_with_asset_base(source, sample_rate, None)
}

pub fn compile_lisp_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<String, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    // Unique name per compile so dlopen doesn't return a stale cached handle
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("effect_{}", seq);

    let src_path = dir.join("effect.lisp");
    let source_with_preamble = format!("{}\n\n{source}", effect_preamble(sample_rate));
    std::fs::write(&src_path, source_with_preamble)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp");
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", &dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()]);
    if let Some(asset_base) = asset_base {
        command.args(["--asset-base", asset_base.to_str().unwrap_or(".")]);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error = format!("{}{}", stderr, stdout);
        log_dgenlisp_compile_failure("effect", &src_path, &error, source);
        return Err(error);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    log_dgenlisp_compile_manifest("effect", &src_path, &stdout);
    Ok(stdout)
}

// ── Parse manifest ──

fn parse_dgen_param_span(param: &serde_json::Value) -> usize {
    const DEFAULT_DGEN_PARAM_SPAN: usize = 1;
    const MAX_DGEN_PARAM_SPAN: usize = 64;

    [
        "cellSpan",
        "vectorWidth",
        "cellWidth",
        "laneWidth",
        "laneCount",
        "span",
        "width",
    ]
    .iter()
    .find_map(|key| param.get(*key).and_then(|value| value.as_u64()))
    .map(|span| span as usize)
    .filter(|span| *span > 0)
    .unwrap_or(DEFAULT_DGEN_PARAM_SPAN)
    .min(MAX_DGEN_PARAM_SPAN)
}

pub fn parse_manifest(json: &str) -> Result<DGenManifest, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse manifest: {e}"))?;

    let dir = output_dir();
    let dylib_name = v["dylib"].as_str().unwrap_or("effect.dylib");
    let dylib_path = dir.join(dylib_name);
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
    desc.params
        .extend(crate::voice_modulator::ui_param_descriptors());

    append_dgen_modulator_descriptors(&mut desc, manifest);
    append_dgen_modulation_target_params(&mut desc, manifest);

    desc
}

pub fn effect_has_host_modulation(manifest: &DGenManifest) -> bool {
    !manifest.mod_destinations.is_empty()
}

fn normalized_dgen_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn is_named_sidechain_input(name: &str) -> bool {
    normalized_dgen_name(name).starts_with("sidechain")
}

fn sidechain_control_name(name: &str) -> String {
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

fn append_dgen_modulator_descriptors(
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

fn append_dgen_modulation_target_params(
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
/// [slot_id, total_memory_slots, canary, declared_input_count, enabled, num_entries, idx0, val0, ...]
/// The engine zeroes state; init only needs to set non-zero values.
fn build_init_message(slot_id: usize, manifest: &DGenManifest) -> Vec<f32> {
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

    // Header (6) + pairs (2 * N)
    let mut msg = Vec::with_capacity(6 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Add effect to track's audio chain ──

/// Remove an effect from the chain and reconnect predecessor → successor.
pub unsafe fn remove_effect_from_chain(
    lg: *mut LiveGraph,
    effect_node_id: i32,
    predecessor_id: i32,
    successor_id: i32,
) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            audiograph::graph_disconnect(lg, predecessor_id, src_port, effect_node_id, dst_port);
            audiograph::graph_disconnect(lg, effect_node_id, src_port, successor_id, dst_port);
            audiograph::graph_disconnect(lg, predecessor_id, src_port, successor_id, dst_port);
        }
    }
    audiograph::delete_node(lg, effect_node_id);
}

pub unsafe fn remove_effect_modulator(lg: *mut LiveGraph, modulator_node_id: i32) {
    if modulator_node_id > 0 {
        audiograph::delete_node(lg, modulator_node_id);
    }
}

unsafe fn disconnect_direct_chain(lg: *mut LiveGraph, predecessor_id: i32, successor_id: i32) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            audiograph::graph_disconnect(lg, predecessor_id, src_port, successor_id, dst_port);
        }
    }
}

unsafe fn connect_effect_port(
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

unsafe fn connect_effect_chain(
    lg: *mut LiveGraph,
    predecessor_id: i32,
    predecessor_outputs: usize,
    effect_id: i32,
    effect_inputs: usize,
    effect_outputs: usize,
    successor_id: i32,
    successor_inputs: usize,
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

    if effect_outputs <= 1 {
        let succ_channels = successor_inputs.max(1).min(2);
        for dst_port in 0..succ_channels {
            connect_effect_port(
                lg,
                effect_id,
                0,
                successor_id,
                dst_port as i32,
                "connect effect output",
            )?;
        }
    } else {
        let succ_channels = successor_inputs.max(1).min(2);
        for ch in 0..succ_channels.min(effect_outputs).min(2) {
            connect_effect_port(
                lg,
                effect_id,
                ch as i32,
                successor_id,
                ch as i32,
                "connect effect output",
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
    // Full state allocation (header + distinct read/write buffers), zeroed by the engine
    let state_size =
        dgen_total_state_slots(manifest.total_memory_slots) * std::mem::size_of::<f32>();

    // Compact init message: only header + non-zero index/value pairs
    let init_msg = build_init_message(slot_id, manifest);
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

    // Commit the replacement only after the new node exists and can be wired.
    // Until this batch succeeds, the old node and process function remain the
    // rollback target for the live graph.
    let previous_process_fn = dgen_process_fn_raw(slot_id);
    audiograph::begin_graph_edit_batch(lg);
    set_dgen_process_fn(slot_id, lib.process_fn);
    let connect_result = connect_effect_chain(
        lg,
        predecessor_id,
        predecessor_outputs,
        node_id,
        manifest.n_inputs,
        manifest.n_outputs,
        successor_id,
        successor_inputs,
    );
    if let Err(error) = connect_result {
        set_dgen_process_fn_raw(slot_id, previous_process_fn);
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
        set_dgen_process_fn_raw(slot_id, previous_process_fn);
        audiograph::delete_node(lg, node_id);
        if let Some(mod_id) = modulator_node_id {
            audiograph::delete_node(lg, mod_id);
        }
        audiograph::end_graph_edit_batch(lg);
        return Err(error);
    }

    if let Some(old_id) = existing_effect {
        remove_effect_from_chain(lg, old_id, predecessor_id, successor_id);
    } else {
        disconnect_direct_chain(lg, predecessor_id, successor_id);
    }
    if let Some(old_mod_id) = existing_modulator {
        remove_effect_modulator(lg, old_mod_id);
    }
    audiograph::end_graph_edit_batch(lg);

    Ok(EffectGraphNodeIds {
        effect_node_id: node_id,
        modulator_node_id,
    })
}

// ── Full interactive editor-compile-load flow ──

pub const EFFECT_TEMPLATE: &str = r#"; DGenLisp stereo effect
;
; Params: (def name (param name @min 0 @max 1 @default 0.5))
; Modulatable: add @mod true @mod-mode additive
;   then use (mod name) to read the modulated value
; Delay:  (def h (history N)), (read-history h delay_samples), (write-history h sample)
; Math:   +, -, *, /, sin, cos, tan, atan, atan2, tanh, clamp, min, max, mix
; Filters: (onepole input coeff)

(def input_l (in 1 @name Left))
(def input_r (in 2 @name Right))
(def mix-amt (param mix @min 0 @max 1 @default 0.5))

; -- Your processing here --
(def processed_l input_l)
(def processed_r input_r)

; -- Stereo output --
(out (mix input_l processed_l mix-amt) 1 @name Left)
(out (mix input_r processed_r mix-amt) 2 @name Right)
"#;

pub struct LispEditResult {
    pub node_id: i32,
    pub lib: LoadedDGenLib,
    pub source: String,
    pub manifest: DGenManifest,
    pub name: String,
}

/// Run the full edit → compile → load → wire → name → save flow.
/// Called while terminal is in normal (non-raw) mode.
pub fn run_editor_flow(
    lg: *mut LiveGraph,
    slot_id: usize,
    track_name: &str,
    predecessor_id: i32,
    successor_id: i32,
    existing_effect: Option<i32>,
    last_source: &str,
    existing_name: Option<&str>,
    sample_rate: u32,
) -> Option<LispEditResult> {
    let initial = if last_source.is_empty() {
        EFFECT_TEMPLATE.to_string()
    } else {
        last_source.to_string()
    };

    let mut source = initial;

    loop {
        // Open editor
        match edit_text(&source) {
            Ok(edited) => {
                source = edited;
            }
            Err(e) => {
                eprintln!("Editor error: {e}");
                return None;
            }
        }

        // Compile
        print!("Compiling...");
        io::stdout().flush().ok();

        match compile_lisp(&source, sample_rate) {
            Ok(json) => {
                match parse_manifest(&json) {
                    Ok(manifest) => {
                        match load_dylib(&manifest.dylib_path) {
                            Ok(lib) => {
                                // Add to graph
                                match unsafe {
                                    add_effect_to_chain_at(
                                        lg,
                                        slot_id,
                                        &manifest,
                                        &lib,
                                        predecessor_id,
                                        2,
                                        successor_id,
                                        2,
                                        existing_effect,
                                        None,
                                        None,
                                    )
                                } {
                                    Ok(node_ids) => {
                                        println!(" OK!");
                                        let n = manifest.params.len();
                                        if n > 0 {
                                            println!("  Parameters:");
                                            for p in &manifest.params {
                                                println!(
                                                    "    {} = {} [{}, {}]{}",
                                                    p.name,
                                                    p.default,
                                                    p.min,
                                                    p.max,
                                                    p.unit
                                                        .as_deref()
                                                        .map(|u| format!(" {u}"))
                                                        .unwrap_or_default()
                                                );
                                            }
                                        }

                                        // Name prompt
                                        let default_name = existing_name.unwrap_or("");
                                        if default_name.is_empty() {
                                            print!("\nEffect name: ");
                                        } else {
                                            print!("\nEffect name [{}]: ", default_name);
                                        }
                                        io::stdout().flush().ok();
                                        let mut name_buf = String::new();
                                        std::io::stdin().read_line(&mut name_buf).ok();
                                        let name_input = name_buf.trim();
                                        let name = if name_input.is_empty() {
                                            if default_name.is_empty() {
                                                "untitled".to_string()
                                            } else {
                                                default_name.to_string()
                                            }
                                        } else {
                                            sanitize_effect_name(name_input)
                                        };

                                        // Save to effects/ library
                                        match save_effect(&name, &source) {
                                            Ok(()) => println!("Saved to effects/{}.lisp", name),
                                            Err(e) => eprintln!("Warning: failed to save: {e}"),
                                        }

                                        println!(
                                            "\nEffect '{}' added to track '{}'",
                                            name, track_name
                                        );
                                        println!("Press Enter to return to sequencer...");
                                        let mut buf = String::new();
                                        std::io::stdin().read_line(&mut buf).ok();
                                        return Some(LispEditResult {
                                            node_id: node_ids.effect_node_id,
                                            lib,
                                            source,
                                            manifest,
                                            name,
                                        });
                                    }
                                    Err(e) => {
                                        eprintln!(" Failed to add to graph: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!(" Failed to load dylib: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(" Failed to parse manifest: {e}");
                    }
                }
            }
            Err(e) => {
                println!();
                eprintln!("Compile error:\n{e}");
            }
        }

        // On any error, offer to re-edit
        eprint!("\nPress Enter to re-edit, or 'q' + Enter to cancel: ");
        io::stdout().flush().ok();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
        if buf.trim() == "q" {
            return None;
        }
    }
}

fn sanitize_effect_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn sanitize_symbol_name(name: &str, uppercase: bool) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        let mapped = if ch.is_alphanumeric() {
            if uppercase {
                ch.to_ascii_uppercase()
            } else {
                ch.to_ascii_lowercase()
            }
        } else {
            '_'
        };
        out.push(mapped);
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

// ══════════════════════════════════════════════════════════════════
// Instrument (synth) support — parallel to effect infrastructure
// ══════════════════════════════════════════════════════════════════

use crate::voice::MAX_VOICES;

#[derive(Clone, Serialize, Deserialize)]
pub struct InstrumentPreset {
    pub id: String,
    pub name: String,
    pub base_note_offset: f32,
    pub params: std::collections::BTreeMap<String, f32>,
}

#[derive(Serialize, Deserialize)]
struct InstrumentMetadataFile {
    version: u32,
    run_mode: String,
}

#[derive(Serialize, Deserialize)]
struct InstrumentPresetBank {
    version: u32,
    engine_name: String,
    source_file: String,
    presets: Vec<InstrumentPreset>,
}

fn resolve_instrument_storage_path(name: &str, extension: &str) -> io::Result<PathBuf> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect_file_matches(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                collect_file_matches(&path, file_name, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
                out.push(path);
            }
        }
    }

    let root = Path::new(INSTRUMENTS_DIR);
    let trimmed = name.trim_end_matches('/');
    if extension == "lisp" && name.ends_with('/') {
        let dsp = root.join(trimmed).join("dsp.lisp");
        if dsp.exists() {
            return Ok(dsp);
        }
    }
    let exact = root.join(format!("{name}.{extension}"));
    if exact.exists() {
        return Ok(exact);
    }
    if extension == "lisp" {
        let dsp = root.join(name).join("dsp.lisp");
        if dsp.exists() {
            return Ok(dsp);
        }
    }

    let basename = Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(trimmed);
    let mut matches = Vec::new();
    if extension == "lisp" {
        collect_folder_source_matches(root, basename, &mut matches);
    }
    let file_name = format!("{basename}.{extension}");
    collect_file_matches(root, &file_name, &mut matches);
    matches.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    matches.dedup();

    match matches.len() {
        0 => Ok(exact),
        1 => Ok(matches.remove(0)),
        _ => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Ambiguous instrument '{name}': found multiple matching instrument sources under {INSTRUMENTS_DIR}"
            ),
        )),
    }
}

fn collect_folder_source_matches(dir: &Path, folder_name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if name == folder_name {
            let dsp = path.join("dsp.lisp");
            if dsp.exists() {
                out.push(dsp);
            }
        }
        collect_folder_source_matches(&path, folder_name, out);
    }
}

fn resolve_instrument_folder_path(name: &str) -> io::Result<PathBuf> {
    let source = resolve_instrument_storage_path(name, "lisp")?;
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Resolved folder-style instrument '{name}' has no parent directory"),
            )
        })
    } else {
        Ok(source.with_extension(""))
    }
}

pub fn instrument_source_path(name: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path(name, "lisp")
}

fn instrument_metadata_path_for_source_path(source: &Path) -> io::Result<PathBuf> {
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source
            .parent()
            .map(|parent| parent.join("instrument.json"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Resolved folder-style instrument source '{}' has no parent directory",
                        source.display()
                    ),
                )
            })
    } else {
        Ok(source.with_extension("instrument.json"))
    }
}

pub fn instrument_metadata_path(name: &str) -> io::Result<PathBuf> {
    let source = instrument_source_path(name)?;
    instrument_metadata_path_for_source_path(&source)
}

pub fn load_instrument_run_mode(name: &str) -> io::Result<CustomInstrumentRunMode> {
    let path = instrument_metadata_path(name)?;
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CustomInstrumentRunMode::Instrument);
        }
        Err(error) => return Err(error),
    };
    let metadata: InstrumentMetadataFile = serde_json::from_str(&source).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse instrument metadata '{}': {error}",
                path.display()
            ),
        )
    })?;
    CustomInstrumentRunMode::parse(&metadata.run_mode).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid instrument run_mode '{}'", metadata.run_mode),
        )
    })
}

pub fn save_instrument_run_mode(name: &str, run_mode: CustomInstrumentRunMode) -> io::Result<()> {
    let path = instrument_metadata_path(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let metadata = InstrumentMetadataFile {
        version: 1,
        run_mode: run_mode.as_str().to_string(),
    };
    let json = serde_json::to_string_pretty(&metadata).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to encode instrument metadata: {error}"),
        )
    })?;
    std::fs::write(path, format!("{json}\n"))
}

fn instrument_name_from_source_path(path: &Path) -> Option<String> {
    if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        if let Some(parent) = path.parent() {
            if let Ok(rel) = parent.strip_prefix(INSTRUMENTS_DIR) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !rel.is_empty() {
                    return Some(format!("{rel}/"));
                }
            }
        }
    }

    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
}

fn source_name_from_path(kind: &CompileKind, path: &Path) -> Option<String> {
    match kind {
        CompileKind::Instrument => instrument_name_from_source_path(path),
        CompileKind::Effect => {
            if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
                path.parent()
                    .and_then(|parent| parent.strip_prefix(EFFECTS_DIR).ok())
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            } else {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
            }
        }
    }
}

fn instrument_preset_path(name: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path(name, "presets")
}

pub fn load_instrument_presets(name: &str) -> io::Result<Vec<InstrumentPreset>> {
    let path = instrument_preset_path(name)?;
    match std::fs::read_to_string(&path) {
        Ok(src) => {
            let bank: InstrumentPresetBank = serde_json::from_str(&src).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse preset bank '{}': {e}", path.display()),
                )
            })?;
            Ok(bank.presets)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn save_instrument_presets(name: &str, presets: &[InstrumentPreset]) -> io::Result<()> {
    let path = instrument_preset_path(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bank = InstrumentPresetBank {
        version: 1,
        engine_name: name.to_string(),
        source_file: format!("instruments/{name}.lisp"),
        presets: presets.to_vec(),
    };
    let json = serde_json::to_string_pretty(&bank).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to serialize preset bank '{}': {e}", path.display()),
        )
    })?;
    std::fs::write(path, json)
}

const INSTRUMENT_REGISTRY_SIZE: usize = MAX_TRACKS * MAX_VOICES;
static DGEN_INSTRUMENT_FNS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
static DGEN_INSTRUMENT_OUTPUT_COUNTS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
static DGEN_ENGINE_ENABLED_VOICES: [AtomicUsize; MAX_TRACKS] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; MAX_TRACKS]
};
static DGEN_ENGINE_PROCESS_CALLS: [AtomicU64; MAX_TRACKS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_TRACKS]
};
static DGEN_ENGINE_PROCESS_BLOCKS: [AtomicU64; MAX_TRACKS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_TRACKS]
};

#[derive(Clone, Copy, Debug)]
pub struct DGenEngineProcessStats {
    pub engine_id: usize,
    pub enabled_voices: usize,
    pub process_calls: u64,
    pub process_blocks: u64,
}

pub fn set_dgen_instrument_fn(slot_id: usize, f: DGenProcessFn) {
    DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].store(f as usize, Ordering::Release);
}

pub fn set_dgen_instrument_output_count(slot_id: usize, count: usize) {
    DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
        .store(count.max(1), Ordering::Release);
}

pub fn set_dgen_engine_enabled_voices(engine_id: usize, count: usize) {
    if engine_id < MAX_TRACKS {
        DGEN_ENGINE_ENABLED_VOICES[engine_id].store(count.clamp(1, MAX_VOICES), Ordering::Release);
    }
}

pub fn get_dgen_engine_enabled_voices(engine_id: usize) -> usize {
    if engine_id < MAX_TRACKS {
        DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .clamp(1, MAX_VOICES)
    } else {
        1
    }
}

pub fn reset_dgen_engine_enabled_voices(engine_id: usize) {
    set_dgen_engine_enabled_voices(engine_id, 1);
}

pub fn take_dgen_engine_process_stats() -> Vec<DGenEngineProcessStats> {
    (0..MAX_TRACKS)
        .map(|engine_id| DGenEngineProcessStats {
            engine_id,
            enabled_voices: get_dgen_engine_enabled_voices(engine_id),
            process_calls: DGEN_ENGINE_PROCESS_CALLS[engine_id].swap(0, Ordering::AcqRel),
            process_blocks: DGEN_ENGINE_PROCESS_BLOCKS[engine_id].swap(0, Ordering::AcqRel),
        })
        .collect()
}

/// Wrapper process function for instrument nodes — reads from DGEN_INSTRUMENT_FNS.
unsafe extern "C" fn dgenlisp_instrument_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let s = state as *mut f32;
    let slot_id = (*s) as usize;
    if slot_id >= INSTRUMENT_REGISTRY_SIZE {
        return;
    }
    if (*s.add(2)).to_bits() != HEADER_CANARY.to_bits() {
        return;
    }
    if *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5 {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
        if !out.is_null() {
            for ch in 0..output_count {
                let out_ch = *out.add(ch);
                if !out_ch.is_null() {
                    for i in 0..nf {
                        *out_ch.add(i) = 0.0;
                    }
                }
            }
        }
        return;
    }
    let engine_id = slot_id / MAX_VOICES;
    let voice_idx = slot_id % MAX_VOICES;
    if engine_id < MAX_TRACKS {
        let enabled = DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .clamp(1, MAX_VOICES);
        if voice_idx >= enabled {
            let nf = nframes as usize;
            let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
                .load(Ordering::Acquire)
                .max(1);
            if !out.is_null() {
                for ch in 0..output_count {
                    let out_ch = *out.add(ch);
                    if !out_ch.is_null() {
                        for i in 0..nf {
                            *out_ch.add(i) = 0.0;
                        }
                    }
                }
            }
            return;
        }
    }
    let fn_ptr = DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let total_memory_slots = *s.add(1) as usize;
        let memory_read = dgen_read_buffer_ptr(s) as *mut c_void;
        let memory_write = dgen_write_buffer_ptr(s, total_memory_slots) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        if (*out.add(0)).is_null() {
            return;
        }
        if engine_id < MAX_TRACKS {
            DGEN_ENGINE_PROCESS_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
            if voice_idx == 0 {
                DGEN_ENGINE_PROCESS_BLOCKS[engine_id].fetch_add(1, Ordering::Relaxed);
            }
        }
        process_fn(
            inp,
            out,
            nframes,
            memory_read,
            memory_write,
            dgen_host_sample_rate(s),
        );
    } else {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
        if !out.is_null() {
            for ch in 0..output_count {
                let out_ch = *out.add(ch);
                if !out_ch.is_null() {
                    for i in 0..nf {
                        *out_ch.add(i) = 0.0;
                    }
                }
            }
        }
    }
}

pub fn dgenlisp_instrument_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_instrument_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
    }
}

/// Build init message for a voice-aware instrument node.
/// Sets slot_id, total_memory_slots, param defaults, tensor data,
/// and voice_cell_id = voice_index.
pub fn build_init_message_for_voice(
    slot_id: usize,
    manifest: &DGenManifest,
    voice_index: usize,
) -> Vec<f32> {
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

    // Set voice cell to voice_index
    if let Some(cell) = manifest.voice_cell_id {
        if cell < manifest.total_memory_slots {
            entries.push((cell, voice_index as f32));
        }
    }

    let mut msg = Vec::with_capacity(6 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Instrument storage ──

pub fn save_instrument(name: &str, source: &str) -> io::Result<()> {
    let path = if name.ends_with('/') {
        Path::new(INSTRUMENTS_DIR)
            .join(name.trim_end_matches('/'))
            .join("dsp.lisp")
    } else {
        resolve_instrument_storage_path(name, "lisp")?
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
}

pub fn save_instrument_ui(name: &str, source: &str) -> io::Result<()> {
    let path = instrument_ui_path(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
}

pub fn instrument_ui_path(name: &str) -> io::Result<PathBuf> {
    if name.ends_with('/') {
        let direct = Path::new(INSTRUMENTS_DIR)
            .join(name.trim_end_matches('/'))
            .join("ui.lisp");
        if direct.exists() {
            return Ok(direct);
        }
    } else {
        let direct = Path::new(INSTRUMENTS_DIR).join(name).join("ui.lisp");
        if direct.exists() {
            return Ok(direct);
        }
    }
    Ok(resolve_instrument_folder_path(name)?.join("ui.lisp"))
}

pub fn load_instrument_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(instrument_ui_path(name)?)
}

pub fn list_saved_instruments() -> Vec<String> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(format!("{}/", rel.to_string_lossy().replace('\\', "/")));
                    }
                }
                collect(&path, root, out);
            } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                let file_stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if matches!(file_stem, "dsp" | "ui" | "presets") {
                    continue;
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    let without_ext = rel.with_extension("");
                    out.push(without_ext.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    let dir = Path::new(INSTRUMENTS_DIR);
    let mut names = Vec::new();
    collect(dir, dir, &mut names);
    names.sort_by_key(|name| name.to_lowercase());
    names
}

pub fn load_instrument_source(name: &str) -> io::Result<String> {
    let path = resolve_instrument_storage_path(name, "lisp")?;
    std::fs::read_to_string(&path)
}

// ── Instrument compilation ──

const INSTRUMENT_PREAMBLE: &str = r#"; Shared instrument helpers injected at compile time.
; `samplerate` is provided by DGenLisp as runtime host sample-rate context.

(defmacro mod_unipolar (m)
  (* (+ m 1.0) 0.5))

(defmacro apply_pitch_mod_semi (base_hz mod amt_semi)
  (def ln2 (log 2))
  (* base_hz (exp (* ln2 (/ (* mod amt_semi) 12)))))

(defmacro apply_cutoff_mod_safe (base mod amt)
  (min 11000 (max 60 (+ base (* mod amt)))))

(defmacro apply_pw_mod_safe (base mod amt)
  (clip (+ base (* mod amt)) 0.03 0.97))

; PolyBLEP transition correction for anti-aliased hard edges.
; Kept with a polypleb alias because that typo is memorable and fun.
(defmacro polyblep (phase freq)
  (def dt (clip (/ freq samplerate) 0.000001 0.5))
  (def left_x (/ phase dt))
  (def left (+ (- (* 2.0 left_x) (* left_x left_x)) -1.0))
  (def right_x (/ (- phase 1.0) dt))
  (def right (+ (* right_x right_x) (* 2.0 right_x) 1.0))
  (+ (* (lt phase dt) left)
     (* (gt phase (- 1.0 dt)) right)))

(defmacro polypleb (phase freq)
  (polyblep phase freq))

(defmacro polyblep_saw (phase freq)
  (- (scale phase 0 1 -1 1)
     (polyblep phase freq)))

(defmacro polyblep_pulse (phase width freq)
  (def w (clip width 0.01 0.99))
  (def falling_phase (wrap (- phase w) 0 1))
  (+ (scale (lt phase w) 0 1 -1 1)
     (polyblep phase freq)
     (* -1.0 (polyblep falling_phase freq))))

; Wavetable helpers assume tensor shape [samples, waves], matching DGenLisp
; peek's (index, channel) convention.
(defmacro wavetable-read-512 (table wave phase)
  (peek table (* (wrap phase 0 1) 512) wave))

(defmacro wavetable-morph-512 (table wave_a wave_b phase morph)
  (wavetable-read-512 table (+ wave_a (* (clip morph 0 1) (- wave_b wave_a))) phase))

; Cytomic-style ZDF state variable filter.
; cutoff in Hz, q is resonance (0.5 = no resonance, higher = more).
; mode: 0=LP, 1=BP, 2=HP, 3=notch, 4=peak, 5=allpass.
(defmacro svf (input cutoff q mode)
  (def safe_cutoff (clip cutoff 1.0 (* samplerate 0.49)))
  (def safe_q (max q 0.001))
  (def g (tan (* pi (/ safe_cutoff samplerate))))
  (def k (/ 1.0 safe_q))
  (def a1 (/ 1.0 (+ 1.0 (* g (+ g k)))))
  (def a2 (* g a1))
  (def a3 (* g a2))

  (make-history ic1eq)
  (make-history ic2eq)

  (def ic1 (read-history ic1eq))
  (def ic2 (read-history ic2eq))
  (def v3 (- input ic2))
  (def v1 (+ (* a1 ic1) (* a2 v3)))
  (def v2 (+ ic2 (* a2 ic1) (* a3 v3)))

  (write-history ic1eq (- (* 2.0 v1) ic1))
  (write-history ic2eq (- (* 2.0 v2) ic2))

  (def lp v2)
  (def bp v1)
  (def hp (- input (* k v1) v2))
  (def notch (+ hp lp))
  (def peak (- lp hp))
  (def ap (- notch (* k v1)))

  (+ (* (eq mode 0) lp)
     (* (eq mode 1) bp)
     (* (eq mode 2) hp)
     (* (eq mode 3) notch)
     (* (eq mode 4) peak)
     (* (eq mode 5) ap)))

; ZDF Moog ladder filter, 4-pole, with input drive, tanh feedback saturation,
; and resonance-proportional passband gain compensation.
; cutoff in Hz, res is 0..1, drive pre-saturates the input.
(defmacro ladder (input cutoff res drive)
  (def wd (* twopi cutoff))
  (def T (/ 1 samplerate))
  (def wa (* (/ 2.0 T) (tan (* wd T 0.5))))
  (def g (* wa T 0.5))
  (def G (/ g (+ 1 g)))
  (def G4 (* G G G G))
  (def k (* res 4))

  (def fb_trim 0.5)

  (make-history z1)
  (make-history z2)
  (make-history z3)
  (make-history z4)

  (def hz1 (read-history z1))
  (def hz2 (read-history z2))
  (def hz3 (read-history z3))
  (def hz4 (read-history z4))
  (def inv_1pg (/ 1 (+ 1 g)))
  (def S (+ (* hz1 G G G inv_1pg)
            (* hz2 G G inv_1pg)
            (* hz3 G inv_1pg)
            (* hz4 inv_1pg)))

  (def driven_input (tanh (* drive input)))
  (def u (/ (- driven_input (* k fb_trim S))
            (+ 1 (* k fb_trim G4))))
  (def x1 (- u (* k (tanh (* fb_trim (+ (* G4 u) S))))))

  (def v1 (* (- x1 hz1) G))
  (def y1 (+ v1 hz1))
  (write-history z1 (+ y1 v1))

  (def v2 (* (- y1 hz2) G))
  (def y2 (+ v2 hz2))
  (write-history z2 (+ y2 v2))

  (def v3 (* (- y2 hz3) G))
  (def y3 (+ v3 hz3))
  (write-history z3 (+ y3 v3))

  (def v4 (* (- y3 hz4) G))
  (def y4 (+ v4 hz4))
  (write-history z4 (+ y4 v4))

  (+ y4 (* res 0.0013 input)))

(defmacro adsr (gate_sig trigger_sig attack_ms decay_ms sustain release_ms)
  (make-history env)
  (make-history gate_hist)
  (make-history stage_hist)

  ; Retriggers first fade any leftover voice history to silence over a
  ; short de-click window, then start a linear attack from near zero.
  ; Decay/release are one-pole curves scaled to settle near the target
  ; over the requested number of milliseconds.
  (def sr samplerate)
  (def env_time_scale 6.907755)
  (def reset_samples (* 0.003 sr))
  (def attack_samples (max 1.0 (* attack_ms 0.001 sr)))
  (def decay_samples (max 1.0 (* decay_ms 0.001 sr)))
  (def release_samples (max 1.0 (* release_ms 0.001 sr)))
  (def reset_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) reset_samples))))
  (def decay_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) decay_samples))))
  (def release_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) release_samples))))

  (def prev_env (read-history env))
  (def prev_gate (read-history gate_hist))
  (def prev_stage (read-history stage_hist))

  (def gate_on (gt gate_sig 0.5))
  (def gate_rising (* gate_on (lte prev_gate 0.5)))
  (def retrigger (max gate_rising trigger_sig))
  (def attack_stage 1.0)
  (def decay_stage 2.0)
  (def reset_stage 3.0)
  (def attack_done (gte prev_env 0.999))
  (def reset_done (lte prev_env 0.0001))

  (def stage_from_gate
    (gswitch gate_on
      (gswitch retrigger
        (gswitch (gt prev_env 0.0001) reset_stage attack_stage)
        prev_stage)
      0.0))

  (def stage
    (gswitch (eq stage_from_gate reset_stage)
      (gswitch reset_done attack_stage reset_stage)
      (gswitch attack_done
        (gswitch (eq stage_from_gate attack_stage) decay_stage stage_from_gate)
        stage_from_gate)))

  (def target
    (gswitch gate_on
      (gswitch (eq stage reset_stage)
        0.0
        (gswitch (eq stage attack_stage) 1.0 sustain))
      0.0))

  (def rate
    (gswitch gate_on
      (gswitch (eq stage reset_stage) reset_coeff decay_coeff)
      release_coeff))

  (def one_pole_level (+ prev_env (* rate (- target prev_env))))
  (def attack_level (+ prev_env (/ 1.0 attack_samples)))
  (def level_raw
    (gswitch (eq stage attack_stage)
      attack_level
      one_pole_level))
  (def level (clip level_raw 0 1))
  (write-history env level)
  (write-history gate_hist gate_sig)
  (write-history stage_hist stage)
  level)
"#;

fn instrument_preamble(sample_rate: u32) -> String {
    let _ = sample_rate;
    INSTRUMENT_PREAMBLE.to_string()
}

fn effect_preamble(sample_rate: u32) -> String {
    instrument_preamble(sample_rate)
}

pub fn compile_instrument(source: &str, sample_rate: u32) -> Result<String, String> {
    compile_instrument_with_asset_base(source, sample_rate, None)
}

pub fn compile_instrument_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<String, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("instrument_{}", seq);

    let src_path = dir.join(format!("instrument_{seq}.lisp"));
    let source_with_preamble = format!("{}\n\n{source}", instrument_preamble(sample_rate));
    std::fs::write(&src_path, source_with_preamble)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp");
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", &dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()])
        .args(["--voices", "12"]);
    if let Some(asset_base) = asset_base {
        command.args(["--asset-base", asset_base.to_str().unwrap_or(".")]);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error = format!("{}{}", stderr, stdout);
        log_dgenlisp_compile_failure("instrument", &src_path, &error, source);
        return Err(error);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    log_dgenlisp_compile_manifest("instrument", &src_path, &stdout);
    Ok(stdout)
}

fn log_dgenlisp_compile_failure(kind: &str, src_path: &Path, error: &str, source: &str) {
    eprintln!(
        "[dgenlisp compile failed] kind={kind} path={}\nerror:\n{error}\nsource:\n{source}\n[/dgenlisp compile failed]",
        src_path.display()
    );
}

fn log_dgenlisp_compile_manifest(kind: &str, src_path: &Path, manifest: &str) {
    /*
    eprintln!(
        "[dgenlisp compile manifest] kind={kind} path={}\nmanifest:\n{manifest}\n[/dgenlisp compile manifest]",
        src_path.display()
    );
    */
}

pub fn compile_and_load_instrument(
    source: &str,
    sample_rate: u32,
) -> Result<CompileResult, String> {
    compile_and_load_instrument_with_asset_base(source, sample_rate, None)
}

pub fn compile_and_load_instrument_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_instrument_with_asset_base(source, sample_rate, asset_base)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult { manifest, lib })
}

pub fn render_instrument_source_for_test(
    source: &str,
    asset_base: Option<&Path>,
    options: &InstrumentRenderOptions,
) -> Result<InstrumentRenderReport, String> {
    let result =
        compile_and_load_instrument_with_asset_base(source, options.sample_rate, asset_base)?;
    render_loaded_instrument_for_test(&result.manifest, &result.lib, options)
}

pub fn render_loaded_instrument_for_test(
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    options: &InstrumentRenderOptions,
) -> Result<InstrumentRenderReport, String> {
    if options.block_size == 0 {
        return Err("block_size must be greater than zero".to_string());
    }
    if options.frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }

    let total_slots = manifest.total_memory_slots;
    let mut memory_read = vec![0.0f32; total_slots];
    let mut memory_write = vec![0.0f32; total_slots];
    let slot_id = options.voice_index;
    let init_msg = build_init_message_for_voice(slot_id, manifest, options.voice_index);
    let entry_count = init_msg.get(5).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[6 + i * 2] as usize;
        let value = init_msg[6 + i * 2 + 1];
        if idx < total_slots {
            memory_read[idx] = value;
        }
    }
    memory_write.copy_from_slice(&memory_read);

    let apply_param = |memory_read: &mut [f32],
                       memory_write: &mut [f32],
                       name: &str,
                       value: f32|
     -> Result<(), String> {
        let param = manifest
            .params
            .iter()
            .find(|param| param.name == name)
            .ok_or_else(|| format!("unknown instrument parameter '{name}'"))?;
        if param.cell_id >= total_slots {
            return Err(format!(
                "parameter '{}' cell {} is outside memory size {}",
                param.name, param.cell_id, total_slots
            ));
        }
        for lane in 0..param.cell_span {
            let idx = param.cell_id + lane;
            if idx < total_slots {
                memory_read[idx] = value;
                memory_write[idx] = value;
            }
        }
        Ok(())
    };

    for (name, value) in &options.param_overrides {
        apply_param(&mut memory_read, &mut memory_write, name, *value)?;
    }

    let mut param_events = options.param_events.clone();
    param_events.sort_by_key(|event| event.frame);
    let mut next_param_event = 0usize;

    let pitch_hz = 440.0 * 2f32.powf((options.midi_note - 69.0) / 12.0);
    let n_inputs = manifest.n_inputs.max(4);
    let n_outputs = manifest.n_outputs.max(1);
    let mut rendered = Vec::with_capacity(options.frames);
    let mut frames_done = 0usize;

    while frames_done < options.frames {
        while next_param_event < param_events.len()
            && param_events[next_param_event].frame <= frames_done
        {
            let event = &param_events[next_param_event];
            apply_param(
                &mut memory_read,
                &mut memory_write,
                &event.name,
                event.value,
            )?;
            next_param_event += 1;
        }

        let next_event_frame = param_events
            .get(next_param_event)
            .map(|event| event.frame)
            .unwrap_or(options.frames)
            .max(frames_done);
        let block_limit = options.block_size.min(options.frames - frames_done);
        let block = block_limit.min((next_event_frame - frames_done).max(1));
        let gate_value = if frames_done < options.gate_frames {
            1.0
        } else {
            0.0
        };
        let trigger_value = if frames_done == 0 { 1.0 } else { 0.0 };

        let mut input_buffers = vec![vec![0.0f32; block]; n_inputs];
        input_buffers[0].fill(gate_value);
        input_buffers[1].fill(pitch_hz);
        input_buffers[2].fill(options.velocity);
        input_buffers[3][0] = trigger_value;
        for &(channel, value) in &options.input_overrides {
            if let Some(buffer) = input_buffers.get_mut(channel) {
                buffer.fill(value);
            }
        }
        let input_ptrs: Vec<*mut f32> = input_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        let mut output_buffers = vec![vec![0.0f32; block]; n_outputs];
        let output_ptrs: Vec<*mut f32> = output_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        unsafe {
            (lib.process_fn)(
                input_ptrs.as_ptr(),
                output_ptrs.as_ptr(),
                block as c_int,
                memory_read.as_mut_ptr() as *mut c_void,
                memory_write.as_mut_ptr() as *mut c_void,
                options.sample_rate.max(1) as c_float,
            );
        }
        rendered.extend_from_slice(&output_buffers[0]);
        frames_done += block;
    }

    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut nonzero_frames = 0usize;
    let mut first_nonzero_frame = None;
    let mut non_finite_samples = 0usize;
    let mut first_non_finite_frame = None;
    for (idx, sample) in rendered.iter().enumerate() {
        if !sample.is_finite() {
            non_finite_samples += 1;
            if first_non_finite_frame.is_none() {
                first_non_finite_frame = Some(idx);
            }
            continue;
        }
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += (*sample as f64) * (*sample as f64);
        sum_abs += abs as f64;
        if abs > 1.0e-7 {
            nonzero_frames += 1;
            if first_nonzero_frame.is_none() {
                first_nonzero_frame = Some(idx);
            }
        }
    }
    let frames = rendered.len().max(1);
    let rms = (sum_sq / frames as f64).sqrt() as f32;
    let mean_abs = (sum_abs / frames as f64) as f32;
    let mut non_finite_state_slots = 0usize;
    let mut first_non_finite_state_slot = None;
    for idx in 0..total_slots {
        if !memory_read[idx].is_finite() || !memory_write[idx].is_finite() {
            non_finite_state_slots += 1;
            if first_non_finite_state_slot.is_none() {
                first_non_finite_state_slot = Some(idx);
            }
        }
    }

    Ok(InstrumentRenderReport {
        frames: rendered.len(),
        peak,
        rms,
        mean_abs,
        nonzero_frames,
        first_nonzero_frame,
        non_finite_samples,
        first_non_finite_frame,
        non_finite_state_slots,
        first_non_finite_state_slot,
        first_samples: rendered.into_iter().take(32).collect(),
    })
}

pub fn render_effect_source_for_test(
    source: &str,
    options: &EffectRenderOptions,
) -> Result<EffectRenderReport, String> {
    let result = compile_and_load(source, options.sample_rate)?;
    render_loaded_effect_for_test(&result.manifest, &result.lib, options)
}

pub fn render_loaded_effect_for_test(
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    options: &EffectRenderOptions,
) -> Result<EffectRenderReport, String> {
    if options.block_size == 0 {
        return Err("block_size must be greater than zero".to_string());
    }
    if options.frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }
    if manifest.n_inputs < 2 || manifest.n_outputs < 2 {
        return Err(format!(
            "effect probe requires at least two inputs and outputs, got {} input(s) and {} output(s)",
            manifest.n_inputs, manifest.n_outputs
        ));
    }

    let total_slots = manifest.total_memory_slots;
    let mut memory_read = vec![0.0f32; total_slots];
    let mut memory_write = vec![0.0f32; total_slots];
    let init_msg = build_init_message(0, manifest);
    let entry_count = init_msg.get(5).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[6 + i * 2] as usize;
        let value = init_msg[6 + i * 2 + 1];
        if idx < total_slots {
            memory_read[idx] = value;
        }
    }
    memory_write.copy_from_slice(&memory_read);

    for (name, value) in &options.param_overrides {
        if let Some(param) = manifest.params.iter().find(|param| param.name == *name) {
            if param.cell_id >= total_slots {
                return Err(format!(
                    "parameter '{}' cell {} is outside memory size {}",
                    param.name, param.cell_id, total_slots
                ));
            }
            for lane in 0..param.cell_span {
                let idx = param.cell_id + lane;
                if idx < total_slots {
                    memory_read[idx] = *value;
                    memory_write[idx] = *value;
                }
            }
            continue;
        }

        let Some(cell_id) = host_mod_descriptor_param_cell(manifest, name) else {
            return Err(format!("unknown effect parameter '{name}'"));
        };
        if cell_id >= total_slots {
            return Err(format!(
                "parameter '{name}' cell {cell_id} is outside memory size {total_slots}"
            ));
        }
        memory_read[cell_id] = *value;
        memory_write[cell_id] = *value;
    }

    let n_inputs = manifest.n_inputs.max(2);
    let n_outputs = manifest.n_outputs.max(2);
    let mut rendered = Vec::with_capacity(options.frames * 2);
    let mut input_reference = Vec::with_capacity(options.frames * 2);
    let mut frames_done = 0usize;

    while frames_done < options.frames {
        let block = options.block_size.min(options.frames - frames_done);
        let mut input_buffers = vec![vec![0.0f32; block]; n_inputs];
        for frame in 0..block {
            let t = (frames_done + frame) as f32 / options.sample_rate.max(1) as f32;
            let impulse = if frames_done + frame == 0 { 0.45 } else { 0.0 };
            let burst_env = (1.0 - (t * 4.0)).max(0.0);
            let left = impulse
                + 0.18
                    * burst_env
                    * ((2.0 * std::f32::consts::PI * 220.0 * t).sin()
                        + 0.5 * (2.0 * std::f32::consts::PI * 997.0 * t).sin());
            let right = 0.12
                * burst_env
                * ((2.0 * std::f32::consts::PI * 330.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 1409.0 * t).sin());
            input_buffers[0][frame] = left;
            input_buffers[1][frame] = right;
            input_reference.push(left);
            input_reference.push(right);
        }
        for &(channel, value) in &options.input_overrides {
            if let Some(buffer) = input_buffers.get_mut(channel) {
                buffer.fill(value);
            }
        }
        let input_ptrs: Vec<*mut f32> = input_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        let mut output_buffers = vec![vec![0.0f32; block]; n_outputs];
        let output_ptrs: Vec<*mut f32> = output_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        unsafe {
            (lib.process_fn)(
                input_ptrs.as_ptr(),
                output_ptrs.as_ptr(),
                block as c_int,
                memory_read.as_mut_ptr() as *mut c_void,
                memory_write.as_mut_ptr() as *mut c_void,
                options.sample_rate.max(1) as c_float,
            );
        }
        for frame in 0..block {
            rendered.push(output_buffers[0][frame]);
            rendered.push(output_buffers[1][frame]);
        }
        frames_done += block;
    }

    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut diff_sq = 0.0f64;
    let mut nonzero_frames = 0usize;
    let mut first_nonzero_frame = None;
    for (idx, sample) in rendered.iter().enumerate() {
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += (*sample as f64) * (*sample as f64);
        sum_abs += abs as f64;
        let input = input_reference.get(idx).copied().unwrap_or(0.0);
        let diff = *sample - input;
        diff_sq += (diff as f64) * (diff as f64);
        if abs > 1.0e-7 {
            nonzero_frames += 1;
            if first_nonzero_frame.is_none() {
                first_nonzero_frame = Some(idx / 2);
            }
        }
    }
    let samples = rendered.len().max(1);
    let rms = (sum_sq / samples as f64).sqrt() as f32;
    let mean_abs = (sum_abs / samples as f64) as f32;
    let diff_rms = (diff_sq / samples as f64).sqrt() as f32;
    let mut left_sq = 0.0f64;
    let mut right_sq = 0.0f64;
    let mut stereo_frames = 0usize;
    for frame in rendered.chunks_exact(2) {
        left_sq += (frame[0] as f64) * (frame[0] as f64);
        right_sq += (frame[1] as f64) * (frame[1] as f64);
        stereo_frames += 1;
    }
    let stereo_frames = stereo_frames.max(1) as f64;
    let left_rms = (left_sq / stereo_frames).sqrt() as f32;
    let right_rms = (right_sq / stereo_frames).sqrt() as f32;

    Ok(EffectRenderReport {
        frames: options.frames,
        peak,
        rms,
        left_rms,
        right_rms,
        mean_abs,
        diff_rms,
        nonzero_frames,
        first_nonzero_frame,
        first_samples: rendered.into_iter().take(32).collect(),
    })
}

fn host_mod_descriptor_param_cell(manifest: &DGenManifest, name: &str) -> Option<usize> {
    for dest in &manifest.mod_destinations {
        if name == format!("__dgen_mod_active__{}", dest.name) {
            return Some(dest.active_cell_id);
        }
        for lane in &dest.depth_lanes {
            if name == format!("mod {} slot {} amt", dest.name, lane.slot) {
                return Some(lane.depth_cell_id);
            }
        }
    }
    None
}

// ── Instrument editor flow ──

pub const INSTRUMENT_TEMPLATE: &str = r#"; DGenLisp instrument
;
; Params:  (param name @default 1.0 @min 0 @max 10)
; Modulatable: add @mod true @mod-mode additive
;   then use (mod name) to read the modulated value
; Envelope: (adsr gate trigger attack_ms decay_ms sustain release_ms)
; Oscillators: (phasor freq_hz), (sin expr), (noise)
; Math: +, -, *, /, sin, cos, tan, atan, atan2, tanh, clamp, min, max
; Constants: twopi, samplerate

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; -- Parameters --
(param attack  @default 5    @min 0   @max 1000 @unit ms)
(param decay   @default 120  @min 1   @max 2000 @unit ms)
(param sustain @default 0.8  @min 0   @max 1)
(param release @default 180  @min 1   @max 5000 @unit ms)
(param gain    @default 0.5  @min 0   @max 1    @mod true @mod-mode additive)

; -- Envelope --
(def env (adsr gate trigger attack decay sustain release))

; -- Oscillator --
(def phase (phasor pitch))

; -- Output --
(out (* phase env velocity (mod gain)) 1 @name audio)
"#;

pub struct InstrumentEditResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub source: String,
    pub params: Vec<DGenParam>,
    pub name: String,
}

pub struct EffectEditResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub source: String,
    pub name: String,
}

struct PendingCompileJob {
    receiver: std::sync::mpsc::Receiver<Result<CompileResult, String>>,
    kind: CompileKind,
    name: String,
    source: String,
}

#[derive(Clone)]
struct LiveAppliedCompile {
    kind: CompileKind,
    name: String,
    source: String,
}

struct RestoreTerminalGuard;

impl Drop for RestoreTerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn editor_file_path(kind: CompileKind, existing_name: Option<&str>) -> PathBuf {
    match kind {
        CompileKind::Instrument => existing_name
            .and_then(|name| instrument_source_path(name).ok())
            .unwrap_or_else(|| {
                Path::new(INSTRUMENTS_DIR)
                    .join(format!("{}.lisp", existing_name.unwrap_or("untitled")))
            }),
        CompileKind::Effect => existing_name
            .map(effect_source_path)
            .unwrap_or_else(|| Path::new(EFFECTS_DIR).join("untitled.lisp")),
    }
}

fn default_template_for_kind(kind: &CompileKind) -> &'static str {
    match kind {
        CompileKind::Instrument => INSTRUMENT_TEMPLATE,
        CompileKind::Effect => EFFECT_TEMPLATE,
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SequencerEvalContext {
    track: usize,
    cursor_step: usize,
}

type SharedSequencerEvalContext = Arc<Mutex<SequencerEvalContext>>;

#[derive(Clone, Debug, Default)]
pub(crate) struct SequencerNativeMetadata {
    effect_descriptors: Vec<Vec<EffectDescriptor>>,
    instrument_descriptors: Vec<EffectDescriptor>,
}

type SharedSequencerNativeMetadata = Arc<Mutex<SequencerNativeMetadata>>;

#[derive(Clone)]
pub(crate) struct RegisteredAccumulator {
    name: String,
    callback: RegisteredAccumulatorCallback,
    params: Vec<crate::effects::ParamDescriptor>,
}

#[derive(Clone)]
enum RegisteredAccumulatorCallback {
    Source(String),
    Closure(EValue),
}

type SharedRegisteredAccumulators = Arc<Mutex<Vec<RegisteredAccumulator>>>;
type SharedRegisteredMidiFx = Arc<Mutex<Vec<RegisteredAccumulator>>>;
type SharedPendingMidiFxParams = Arc<Mutex<Vec<crate::effects::ParamDescriptor>>>;
type SharedMidiFxState = Arc<Mutex<HashMap<String, EValue>>>;

#[derive(Clone)]
pub(crate) struct AccumulatorEvalContext {
    step_index: usize,
    resolved: ResolvedStep,
    chord: Vec<f32>,
    chord_durations: Vec<f32>,
    chord_step_transpose: f32,
    note_spans: Option<Vec<AccumulatorNoteSpan>>,
    midi_fx_scope: Option<(usize, String)>,
    midi_fx_slot: EffectSlotSnapshot,
    midi_fx_param_names: Vec<String>,
    arp_phase_beats: f32,
    step_beats: f32,
    num_steps: usize,
    suppressed: bool,
    effect_slots: Vec<EffectSlotSnapshot>,
    instrument_slot: EffectSlotSnapshot,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: Vec<ScheduledInstrumentParam>,
    emitted: Vec<EmittedAccumulatorEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmittedAccumulatorEvent {
    pub offset_beats: f32,
    pub track: Option<usize>,
    pub resolved: ResolvedStep,
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_step_transpose: f32,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccumulatorNoteSpan {
    pub transpose: f32,
    pub start_beats: f32,
    pub end_beats: f32,
}

#[derive(Clone)]
pub struct AccumulatorEvalOutput {
    pub resolved: ResolvedStep,
    pub suppressed: bool,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
    pub emitted: Vec<EmittedAccumulatorEvent>,
}

type SharedAccumulatorEvalContext = Arc<Mutex<Option<AccumulatorEvalContext>>>;

type SharedRegisteredSequencers = Arc<Mutex<Vec<RegisteredSequencer>>>;
type SharedGeneratorTickContext = Arc<Mutex<Option<GeneratorTickContext>>>;
type SharedGraphNodeContext = Arc<Mutex<Option<GraphNodeContext>>>;

/// Per-node-event context for a graph-mode `:update`, bound while the update body
/// evaluates so the `node-*` accessors read it. Carries only musical/symbolic
/// coordinates and the node's resolved input + behavioral params/state (no samples).
pub(crate) struct GraphNodeContext {
    node_index: usize,
    input: f64,
    energy: f64,
    tick_index: u64,
    beat: f64,
    /// Behavioral params (`node-param`), prototype defaults + per-instance plocks.
    params: HashMap<String, f64>,
    /// Author-defined state cells (`node-state`/`node-set!`) beyond engine `energy`.
    state: HashMap<String, f64>,
    /// The payload that arrived this boundary (`node-input-event`), if any (Ext 1).
    input_event: Option<crate::graph::GraphPayload>,
    dampen_incoming: Option<f64>,
    recover_incoming: Option<f64>,
}

/// A lisp `def-sequencer` definition as held by the scheduler-side VM: its id
/// (stable hash of the name, for hot-reload matching), display name, `:resolution`
/// timebase, and the `:tick` closure to invoke per boundary crossing.
#[derive(Clone)]
pub(crate) struct RegisteredSequencer {
    id: u64,
    name: String,
    resolution: Timebase,
    tick: RegisteredAccumulatorCallback,
}

struct CompiledGraphUpdate {
    source: String,
    callback: EValue,
}

/// Per-invocation context for a generator `:tick`, mirroring [`AccumulatorEvalContext`]
/// but for self-clocked generators: musical position only (no source step), an RNG
/// cell for `gen-rand`, and the buffer that `seq-emit` pushes into.
pub(crate) struct GeneratorTickContext {
    tick_index: u64,
    beat: f64,
    resolution_beats: f64,
    random_state: u64,
    state: HashMap<String, f64>,
    emitted: Vec<EmittedAccumulatorEvent>,
}

pub struct ScratchControlRuntime {
    runtime: Runtime,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
    accumulators: SharedRegisteredAccumulators,
    midi_fx: SharedRegisteredMidiFx,
    pending_midi_fx_params: SharedPendingMidiFxParams,
    midi_fx_state: SharedMidiFxState,
    accumulator_eval: SharedAccumulatorEvalContext,
    sequencers: SharedRegisteredSequencers,
    generator_tick: SharedGeneratorTickContext,
    graph_node: SharedGraphNodeContext,
    graph_updates: HashMap<u64, CompiledGraphUpdate>,
    runtime_globals: Vec<String>,
}

impl ScratchControlRuntime {
    pub fn new(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
    ) -> Self {
        let context = Arc::new(Mutex::new(SequencerEvalContext { track, cursor_step }));
        let metadata = Arc::new(Mutex::new(SequencerNativeMetadata {
            effect_descriptors,
            instrument_descriptors,
        }));
        let accumulators = Arc::new(Mutex::new(Vec::new()));
        let midi_fx = Arc::new(Mutex::new(Vec::new()));
        let pending_midi_fx_params = Arc::new(Mutex::new(Vec::new()));
        let midi_fx_state = Arc::new(Mutex::new(HashMap::new()));
        let accumulator_eval = Arc::new(Mutex::new(None));
        let sequencers = Arc::new(Mutex::new(Vec::new()));
        let generator_tick = Arc::new(Mutex::new(None));
        let graph_node: SharedGraphNodeContext = Arc::new(Mutex::new(None));
        let mut runtime = Runtime::new();
        runtime.set_theme_sync_enabled(false);
        register_sequencer_natives_with_accumulators(
            &mut runtime,
            state,
            Arc::clone(&context),
            Arc::clone(&metadata),
            Arc::clone(&accumulators),
            Arc::clone(&midi_fx),
            Arc::clone(&pending_midi_fx_params),
            Arc::clone(&midi_fx_state),
            Arc::clone(&accumulator_eval),
            Arc::clone(&sequencers),
            Arc::clone(&generator_tick),
        );
        register_graph_node_natives(&mut runtime, Arc::clone(&graph_node));
        let mut this = Self {
            runtime,
            context,
            metadata,
            accumulators,
            midi_fx,
            pending_midi_fx_params,
            midi_fx_state,
            accumulator_eval,
            sequencers,
            generator_tick,
            graph_node,
            graph_updates: HashMap::new(),
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
    }

    pub fn set_position(&mut self, track: usize, cursor_step: usize) {
        if let Ok(mut ctx) = self.context.lock() {
            ctx.track = track;
            ctx.cursor_step = cursor_step;
        }
        self.refresh_runtime_globals();
    }

    pub fn sync_descriptors(
        &mut self,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
    ) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.effect_descriptors = effect_descriptors;
            metadata.instrument_descriptors = instrument_descriptors;
        }
        self.refresh_runtime_globals();
    }

    pub fn eval(&mut self, code: &str) -> Result<Option<EValue>, String> {
        self.runtime.eval_str(code).map_err(|e| format!("{e:?}"))
    }

    pub fn take_status_message(&mut self) -> Option<String> {
        self.runtime.take_status_message()
    }

    pub fn set_theme_sync_enabled(&mut self, enabled: bool) {
        self.runtime.set_theme_sync_enabled(enabled);
    }

    pub fn set_global_value(&mut self, name: &str, value: EValue) {
        self.runtime.set_global_value(name, value);
    }

    fn refresh_runtime_globals(&mut self) {
        self.runtime_globals = install_runtime_globals(
            &mut self.runtime,
            &self.context,
            &self.metadata,
            &self.runtime_globals,
        );
    }

    fn install_accumulator_macro(&mut self) {
        let _ = self.runtime.eval_str(
            r#"
            (defmacro def-accumulator (name body)
              `(__register-accumulator ,name
                 (lambda (acc-step acc-value) ,body)))
            "#,
        );
    }

    fn install_midi_fx_macro(&mut self) {
        let _ = self.runtime.eval_str(
            r#"
            (defmacro def-midi-fx (name body)
              `(__register-midi-fx ,name
                 (lambda (fx-step fx-value) ,body)))
            "#,
        );
    }

    pub fn accumulator_names(&self) -> Vec<String> {
        self.accumulators
            .lock()
            .map(|registry| registry.iter().map(|entry| entry.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn midi_fx_names(&self) -> Vec<String> {
        self.midi_fx
            .lock()
            .map(|registry| registry.iter().map(|entry| entry.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn midi_fx_descriptors(&self) -> Vec<EffectDescriptor> {
        self.midi_fx
            .lock()
            .map(|registry| {
                registry
                    .iter()
                    .map(|entry| {
                        let mut desc = EffectDescriptor::empty_custom_slot();
                        desc.name = entry.name.clone();
                        desc.params = entry.params.clone();
                        desc
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn invoke_accumulator(
        &mut self,
        registry_index: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        let callback = self
            .accumulators
            .lock()
            .map_err(|_| "failed to lock accumulator registry".to_string())?
            .get(registry_index)
            .map(|entry| entry.callback.clone())
            .ok_or_else(|| "registered accumulator out of range".to_string())?;
        {
            let mut eval_ctx = self
                .accumulator_eval
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            *eval_ctx = Some(AccumulatorEvalContext {
                step_index: step,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                note_spans,
                midi_fx_scope: None,
                midi_fx_slot: EffectSlotSnapshot::new_empty(),
                midi_fx_param_names: Vec::new(),
                arp_phase_beats: 0.0,
                step_beats,
                num_steps,
                suppressed: false,
                effect_slots,
                instrument_slot,
                effect_params,
                instrument_params,
                emitted: Vec::new(),
            });
        }
        self.runtime
            .set_global_value("acc-step", EValue::Number(step as f64));
        self.runtime
            .set_global_value("acc-value", EValue::Number(value as f64));
        match callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.runtime
                    .eval_str(&source)
                    .map_err(|e| format!("{e:?}"))?;
            }
            RegisteredAccumulatorCallback::Closure(callback) => {
                self.runtime
                    .invoke(
                        callback,
                        vec![EValue::Number(step as f64), EValue::Number(value as f64)],
                    )
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        let output = self
            .accumulator_eval
            .lock()
            .map_err(|_| "failed to lock accumulator eval context".to_string())?
            .take()
            .ok_or_else(|| "accumulator did not produce an evaluation context".to_string())?;
        Ok(AccumulatorEvalOutput {
            resolved: output.resolved,
            suppressed: output.suppressed,
            effect_params: output.effect_params,
            instrument_params: output.instrument_params,
            emitted: output.emitted,
        })
    }

    pub fn invoke_midi_fx(
        &mut self,
        registry_index: usize,
        track: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        midi_fx_slot: EffectSlotSnapshot,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        self.invoke_midi_fx_with_arp_phase_beats(
            registry_index,
            track,
            step,
            value,
            resolved,
            chord,
            chord_durations,
            chord_step_transpose,
            note_spans,
            midi_fx_slot,
            0.0,
            step_beats,
            num_steps,
            effect_slots,
            instrument_slot,
            effect_params,
            instrument_params,
        )
    }

    pub fn invoke_midi_fx_with_arp_phase_beats(
        &mut self,
        registry_index: usize,
        track: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        midi_fx_slot: EffectSlotSnapshot,
        arp_phase_beats: f32,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        let entry = self
            .midi_fx
            .lock()
            .map_err(|_| "failed to lock MIDI FX registry".to_string())?
            .get(registry_index)
            .cloned()
            .ok_or_else(|| "registered MIDI FX out of range".to_string())?;
        let midi_fx_slot = if midi_fx_slot.num_params == 0 && !entry.params.is_empty() {
            EffectSlotSnapshot::new_default(
                &EffectDescriptor {
                    name: entry.name.clone(),
                    params: entry.params.clone(),
                    input_channels: 0,
                    output_channels: 0,
                    instrument_modulators: Vec::new(),
                    instrument_modulation_targets: Vec::new(),
                },
                0,
            )
        } else {
            midi_fx_slot
        };
        {
            let mut eval_ctx = self
                .accumulator_eval
                .lock()
                .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
            *eval_ctx = Some(AccumulatorEvalContext {
                step_index: step,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                note_spans,
                midi_fx_scope: Some((track, entry.name.clone())),
                midi_fx_slot,
                midi_fx_param_names: entry
                    .params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect(),
                arp_phase_beats,
                step_beats,
                num_steps,
                suppressed: false,
                effect_slots,
                instrument_slot,
                effect_params,
                instrument_params,
                emitted: Vec::new(),
            });
        }
        self.runtime
            .set_global_value("fx-step", EValue::Number(step as f64));
        self.runtime
            .set_global_value("fx-value", EValue::Number(value as f64));
        match entry.callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.runtime
                    .eval_str(&source)
                    .map_err(|e| format!("{e:?}"))?;
            }
            RegisteredAccumulatorCallback::Closure(callback) => {
                self.runtime
                    .invoke(
                        callback,
                        vec![EValue::Number(step as f64), EValue::Number(value as f64)],
                    )
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        let output = self
            .accumulator_eval
            .lock()
            .map_err(|_| "failed to lock MIDI FX eval context".to_string())?
            .take()
            .ok_or_else(|| "MIDI FX did not produce an evaluation context".to_string())?;
        Ok(AccumulatorEvalOutput {
            resolved: output.resolved,
            suppressed: output.suppressed,
            effect_params: output.effect_params,
            instrument_params: output.instrument_params,
            emitted: output.emitted,
        })
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Runtime,
        SharedSequencerEvalContext,
        SharedSequencerNativeMetadata,
        SharedRegisteredAccumulators,
        SharedRegisteredMidiFx,
        SharedPendingMidiFxParams,
        SharedMidiFxState,
        SharedAccumulatorEvalContext,
        SharedRegisteredSequencers,
        SharedGeneratorTickContext,
        SharedGraphNodeContext,
    ) {
        (
            self.runtime,
            self.context,
            self.metadata,
            self.accumulators,
            self.midi_fx,
            self.pending_midi_fx_params,
            self.midi_fx_state,
            self.accumulator_eval,
            self.sequencers,
            self.generator_tick,
            self.graph_node,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        runtime: Runtime,
        context: SharedSequencerEvalContext,
        metadata: SharedSequencerNativeMetadata,
        accumulators: SharedRegisteredAccumulators,
        midi_fx: SharedRegisteredMidiFx,
        pending_midi_fx_params: SharedPendingMidiFxParams,
        midi_fx_state: SharedMidiFxState,
        accumulator_eval: SharedAccumulatorEvalContext,
        sequencers: SharedRegisteredSequencers,
        generator_tick: SharedGeneratorTickContext,
        graph_node: SharedGraphNodeContext,
    ) -> Self {
        let mut this = Self {
            runtime,
            context,
            metadata,
            accumulators,
            midi_fx,
            pending_midi_fx_params,
            midi_fx_state,
            accumulator_eval,
            sequencers,
            generator_tick,
            graph_node,
            graph_updates: HashMap::new(),
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
    }

    fn graph_update_callback(&mut self, id: u64, source: &str) -> Result<EValue, String> {
        if let Some(compiled) = self.graph_updates.get(&id) {
            if compiled.source == source {
                return Ok(compiled.callback.clone());
            }
        }

        let wrapped = format!("(lambda (self) {source})");
        let callback = self
            .runtime
            .eval_str(&wrapped)
            .map_err(|e| format!("failed to compile graph update: {e:?}; source={source}"))?
            .ok_or_else(|| "graph update compilation produced no callback".to_string())?;
        match callback {
            EValue::Closure(_, _) | EValue::NativeFunction(_) => {
                self.graph_updates.insert(
                    id,
                    CompiledGraphUpdate {
                        source: source.to_string(),
                        callback: callback.clone(),
                    },
                );
                Ok(callback)
            }
            other => Err(format!(
                "graph update must compile to a callable, got {}",
                eseqlisp::vm::format_lisp_value(&other)
            )),
        }
    }

    /// Register a generator whose `:tick` is shipped source (from a UI-runtime
    /// `def-sequencer` published via `SequencerState`). Upserts by id so re-evaluating
    /// the authoring file hot-reloads the body without duplicating the generator.
    pub fn register_published_sequencer(
        &self,
        id: u64,
        name: String,
        resolution: Timebase,
        tick_source: String,
    ) {
        if let Ok(mut registry) = self.sequencers.lock() {
            let entry = RegisteredSequencer {
                id,
                name,
                resolution,
                tick: RegisteredAccumulatorCallback::Source(tick_source),
            };
            if let Some(existing) = registry.iter_mut().find(|e| e.id == id) {
                *existing = entry;
            } else {
                registry.push(entry);
            }
        }
    }

    /// Definitions of all generators registered in this VM, for the scheduler to
    /// reconcile into its [`crate::generator::GeneratorRuntime`].
    pub fn sequencer_defs(&self) -> Vec<crate::generator::GeneratorDef> {
        self.sequencers
            .lock()
            .map(|registry| {
                registry
                    .iter()
                    .map(|entry| crate::generator::GeneratorDef {
                        id: entry.id,
                        name: entry.name.clone(),
                        resolution_beats: entry
                            .resolution
                            .step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Invoke a registered generator's `:tick` closure for one boundary crossing,
    /// returning the events it emitted plus the advanced RNG state. Mirrors
    /// [`Self::invoke_accumulator`] but for self-clocked generators.
    pub fn invoke_sequencer_tick(
        &mut self,
        registry_index: usize,
        input: crate::generator::GeneratorTickInput,
    ) -> Result<crate::generator::GeneratorTickResult, String> {
        let callback = self
            .sequencers
            .lock()
            .map_err(|_| "failed to lock sequencer registry".to_string())?
            .get(registry_index)
            .map(|entry| entry.tick.clone())
            .ok_or_else(|| "registered sequencer out of range".to_string())?;
        {
            let mut ctx = self
                .generator_tick
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            *ctx = Some(GeneratorTickContext {
                tick_index: input.tick_index,
                beat: input.beat,
                resolution_beats: input.resolution_beats,
                random_state: input.random_state,
                state: input.state,
                emitted: Vec::new(),
            });
        }
        match callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.runtime
                    .eval_str(&source)
                    .map_err(|e| format!("{e:?}"))?;
            }
            RegisteredAccumulatorCallback::Closure(callback) => {
                self.runtime
                    .invoke(callback, vec![])
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        let ctx = self
            .generator_tick
            .lock()
            .map_err(|_| "failed to lock generator tick context".to_string())?
            .take()
            .ok_or_else(|| "generator tick did not produce a context".to_string())?;
        Ok(crate::generator::GeneratorTickResult {
            emitted: ctx.emitted,
            random_state: ctx.random_state,
            state: ctx.state,
        })
    }

    /// Run a graph node's `:update` rule for one evaluation boundary and report
    /// whether it fired. The behavioral params (prototype defaults; per-instance plocks
    /// later) and the engine-integrated `energy` are bound via the `node-*` accessors;
    /// the truthiness of the body's result is the fire decision. With no `:update`
    /// body, falls back to the neural rule (fire when `energy >= threshold`).
    ///
    /// v1a: `energy` is engine-owned (integrated + reset by [`crate::graph::GraphRuntime`]),
    /// so the body is a pure predicate. Author state cells and emit/relay arrive in v1b.
    pub fn invoke_graph_update(
        &mut self,
        manifest: &crate::graph::GraphManifest,
        eval: &crate::graph::NodeEval,
    ) -> Result<crate::graph::NodeFire, String> {
        let params = eval.params.clone();
        let Some(source) = manifest.node.update_source.as_deref() else {
            let threshold = params.get("threshold").copied().unwrap_or(1.0);
            return Ok(crate::graph::NodeFire {
                fired: eval.energy >= threshold,
                ..crate::graph::NodeFire::default()
            });
        };
        let callback = self.graph_update_callback(manifest.id, source)?;
        {
            let mut ctx = self
                .graph_node
                .lock()
                .map_err(|_| "failed to lock graph node context".to_string())?;
            *ctx = Some(GraphNodeContext {
                node_index: eval.node_index,
                input: eval.input,
                energy: eval.energy,
                tick_index: eval.tick_index,
                beat: eval.beat,
                params,
                state: HashMap::new(),
                input_event: eval.input_event,
                dampen_incoming: None,
                recover_incoming: None,
            });
        }
        let result = self
            .runtime
            .invoke(callback, vec![EValue::Number(eval.node_index as f64)])
            .map_err(|e| format!("{e:?}"));
        if std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some() {
            eprintln!(
                "[graph-update] id={} node={} src={source:?} result={result:?}",
                manifest.id, eval.node_index
            );
        }
        let mut dampen_incoming = None;
        let mut recover_incoming = None;
        if let Ok(mut ctx) = self.graph_node.lock() {
            if let Some(ctx) = ctx.take() {
                dampen_incoming = ctx.dampen_incoming;
                recover_incoming = ctx.recover_incoming;
            }
        }
        let fired = matches!(&result, Ok(Some(v)) if evalue_is_truthy(v));
        let emit = match &result {
            Ok(Some(v)) => parse_emit_spec(v),
            _ => None,
        };
        result?;
        Ok(crate::graph::NodeFire {
            fired,
            emit,
            dampen_incoming,
            recover_incoming,
        })
    }
}

/// Lisp truthiness: everything is true except `false` and `nil`.
fn evalue_is_truthy(value: &EValue) -> bool {
    !matches!(value, EValue::Bool(false) | EValue::Nil)
}

/// Marker key stamped onto the Map that `(emit …)` returns, so a `:update`'s result is
/// distinguishable from any other truthy value (a plain `true`, a number, or an
/// `in-event` map) when [`ScratchControlRuntime::invoke_graph_update`] decodes it.
const EMIT_MARKER: &str = "__emit";

/// Decode the shaped event a `:update` body returned. Only a Map carrying [`EMIT_MARKER`]
/// (i.e. the value `(emit …)` produced) yields an [`crate::graph::EmitSpec`]; any other
/// truthy value means "fire with the legacy default" and returns `None`.
fn parse_emit_spec(value: &EValue) -> Option<crate::graph::EmitSpec> {
    let EValue::Map(map) = value else {
        return None;
    };
    map.get(EMIT_MARKER)?;
    let field = |key: &str| -> Option<f32> {
        map.get(key).and_then(|cell| match &*cell.borrow() {
            EValue::Number(n) => Some(*n as f32),
            _ => None,
        })
    };
    Some(crate::graph::EmitSpec {
        note: field("note"),
        velocity: field("vel"),
    })
}

/// Register the `node-*` accessors a graph-mode `:update` reads. They read the
/// currently-bound [`GraphNodeContext`] (set by `invoke_graph_update`) and ignore
/// their `self` argument (the context is ambient, like the `gen-*` builtins).
fn register_graph_node_natives(runtime: &mut Runtime, graph_node: SharedGraphNodeContext) {
    fn ctx_key(value: Option<&EValue>) -> Option<String> {
        match value {
            Some(EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k)) => {
                Some(k.trim_start_matches(':').to_string())
            }
            _ => None,
        }
    }

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-input",
        "(node-input self)",
        "The reduced gather result arriving at this node this evaluation boundary.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-input called outside :update")?;
            Ok(EValue::Number(ctx.input))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-index",
        "(node-index self)",
        "This node's instance index within the shape.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-index called outside :update")?;
            Ok(EValue::Number(ctx.node_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-tick",
        "(node-tick self)",
        "0-based count of this node's evaluation boundaries since reset.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-tick called outside :update")?;
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-param",
        "(node-param self :key)",
        "Read a behavioral param of this node (prototype default + per-instance plock).",
        move |args, _ctx| {
            let key =
                ctx_key(args.get(1).or_else(|| args.first())).ok_or("node-param expects a key")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-param called outside :update")?;
            Ok(EValue::Number(ctx.params.get(&key).copied().unwrap_or(0.0)))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-state",
        "(node-state self :key)",
        "Read a runtime state cell of this node (engine `energy`, or an author cell).",
        move |args, _ctx| {
            let key =
                ctx_key(args.get(1).or_else(|| args.first())).ok_or("node-state expects a key")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-state called outside :update")?;
            let value = if key == "energy" {
                ctx.energy
            } else {
                ctx.state.get(&key).copied().unwrap_or(0.0)
            };
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-input-event",
        "(node-input-event self)",
        "The payload (event) that arrived at this node this boundary, or nil (Ext 1).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_ref()
                .ok_or("node-input-event called outside :update")?;
            Ok(payload_to_event(ctx.input_event))
        },
    );

    fn event_field(args: &[EValue], key: &str) -> EValue {
        if let Some(EValue::Map(map)) = args.first() {
            if let Some(cell) = map.get(key) {
                if let EValue::Number(n) = &*cell.borrow() {
                    return EValue::Number(*n);
                }
            }
        }
        EValue::Number(0.0)
    }
    runtime.register_native_with_docs(
        "event-note",
        "(event-note ev)",
        "Read the note (transpose) field off a relayed event (Ext 1).",
        move |args, _ctx| Ok(event_field(&args, "note")),
    );
    runtime.register_native_with_docs(
        "event-vel",
        "(event-vel ev)",
        "Read the velocity field off a relayed event (Ext 1).",
        move |args, _ctx| Ok(event_field(&args, "vel")),
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-set!",
        "(node-set! self :key value)",
        "Write an author state cell of this node (v1a: engine `energy` is engine-owned).",
        move |args, _ctx| {
            let key = ctx_key(args.get(1)).ok_or("node-set! expects a key")?;
            let value = match args.get(2) {
                Some(EValue::Number(n)) => *n,
                _ => return Err("node-set! expects (node-set! self :key number)".to_string()),
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_mut().ok_or("node-set! called outside :update")?;
            if key != "energy" {
                ctx.state.insert(key, value);
            }
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "dampen-incoming",
        "(dampen-incoming self amount)",
        "Request dampening for incoming edges that triggered this node if the firing commits.",
        move |args, _ctx| {
            let amount = match args.get(1).or_else(|| args.first()) {
                Some(EValue::Number(n)) => *n,
                _ => {
                    return Err("dampen-incoming expects (dampen-incoming self amount)".to_string());
                }
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_mut()
                .ok_or("dampen-incoming called outside :update")?;
            ctx.dampen_incoming = Some(amount);
            // Returns nil so a `:update` ending on this edge-effect reads as "no fire".
            Ok(EValue::Nil)
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "recover-incoming",
        "(recover-incoming self factor)",
        "Request recovery for all incoming edges if this node does not fire.",
        move |args, _ctx| {
            let factor = match args.get(1).or_else(|| args.first()) {
                Some(EValue::Number(n)) => *n,
                _ => {
                    return Err(
                        "recover-incoming expects (recover-incoming self factor)".to_string()
                    );
                }
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_mut()
                .ok_or("recover-incoming called outside :update")?;
            ctx.recover_incoming = Some(factor);
            // Returns nil so the common `(if fire? (emit …) (recover-incoming …))`
            // shape skips when the else-branch runs (the chosen no-fire form).
            Ok(EValue::Nil)
        },
    );

    // ── Terse, self-less surface (Ext B) ──────────────────────────────────────────
    // The node context is ambient, so these take no `self`. They read/write the same
    // bound `GraphNodeContext` as the `node-*` accessors above; the older `node-*`
    // forms remain as aliases so existing definitions keep working.

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "param",
        "(param :key)",
        "Read a behavioral param of this node (prototype default + per-instance plock).",
        move |args, _ctx| {
            let key = ctx_key(args.first()).ok_or("param expects (param :key)")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("param called outside :update")?;
            Ok(EValue::Number(ctx.params.get(&key).copied().unwrap_or(0.0)))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "energy",
        "(energy)",
        "Read this node's engine-owned integrated energy.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("energy called outside :update")?;
            Ok(EValue::Number(ctx.energy))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "set-state!",
        "(set-state! :key value)",
        "Write an author state cell of this node (engine `energy` is engine-owned).",
        move |args, _ctx| {
            let key = ctx_key(args.first()).ok_or("set-state! expects a key")?;
            let value = match args.get(1) {
                Some(EValue::Number(n)) => *n,
                _ => return Err("set-state! expects (set-state! :key number)".to_string()),
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_mut().ok_or("set-state! called outside :update")?;
            if key != "energy" {
                ctx.state.insert(key, value);
            }
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "input",
        "(input)",
        "The reduced gather result arriving at this node this evaluation boundary.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("input called outside :update")?;
            Ok(EValue::Number(ctx.input))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "index",
        "(index)",
        "This node's instance index within the shape.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("index called outside :update")?;
            Ok(EValue::Number(ctx.node_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "step",
        "(step)",
        "0-based count of this node's evaluation boundaries since reset.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("step called outside :update")?;
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-event",
        "(in-event)",
        "The payload (event) that arrived at this node this boundary, or nil.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-event called outside :update")?;
            Ok(payload_to_event(ctx.input_event))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-note",
        "(in-note)",
        "The note of the event arriving this boundary (0 if nothing arrived).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-note called outside :update")?;
            Ok(EValue::Number(
                ctx.input_event.map(|p| p.note as f64).unwrap_or(0.0),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-vel",
        "(in-vel)",
        "The velocity of the event arriving this boundary (1.0 if nothing arrived).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-vel called outside :update")?;
            Ok(EValue::Number(
                ctx.input_event.map(|p| p.velocity as f64).unwrap_or(1.0),
            ))
        },
    );

    runtime.register_native_with_docs(
        "emit",
        "(emit :note n :vel v)",
        "Fire this node with a shaped event. Each named field overrides the emitted and \
         propagated payload; unnamed fields relay the incoming event verbatim. Returning \
         it from `:update` is the fire decision (truthy).",
        move |args, _ctx| {
            let mut map: HashMap<String, std::rc::Rc<std::cell::RefCell<EValue>>> = HashMap::new();
            map.insert(
                EMIT_MARKER.to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Bool(true))),
            );
            let mut i = 0;
            while i < args.len() {
                let key = ctx_key(args.get(i))
                    .ok_or("emit expects keyword/value pairs, e.g. (emit :note 60 :vel 0.8)")?;
                let value = match args.get(i + 1) {
                    Some(EValue::Number(n)) => *n,
                    _ => return Err(format!("emit field :{key} expects a number")),
                };
                let field = match key.as_str() {
                    "note" => "note",
                    "vel" | "velocity" => "vel",
                    other => return Err(format!("emit: unknown field :{other}")),
                };
                map.insert(
                    field.to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(value))),
                );
                i += 2;
            }
            Ok(EValue::Map(map))
        },
    );
}

/// Build the `{note, vel}` Map an `:update` sees for an arrived payload, or nil.
fn payload_to_event(payload: Option<crate::graph::GraphPayload>) -> EValue {
    match payload {
        Some(payload) => {
            let mut map = HashMap::new();
            map.insert(
                "note".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(payload.note as f64))),
            );
            map.insert(
                "vel".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(
                    payload.velocity as f64,
                ))),
            );
            EValue::Map(map)
        }
        None => EValue::Nil,
    }
}

fn register_sequencer_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
) {
    register_sequencer_natives_with_accumulators(
        runtime,
        state,
        context,
        metadata,
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(None)),
    );
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SelectedNeuralNeuron {
    pub pattern_idx: usize,
    pub network_id: u64,
    pub neuron_idx: usize,
}

pub type SharedSelectedNeuralNeurons = Arc<Mutex<BTreeSet<SelectedNeuralNeuron>>>;

pub fn register_neural_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
) {
    register_neural_authoring_natives_with_selection(
        runtime,
        state,
        Arc::new(Mutex::new(BTreeSet::new())),
    );
}

pub fn register_graph_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
) {
    // Writable mirror of resolved graph values; `bind-graph` reads it, `reactive-set`
    // dirties it. Dynamic-field namespace (no declared fields), like SEQV.
    runtime.register_reactive(GRAPH_REACTIVE_NS, vec![], true);

    let state_for_graph_list = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-list",
        "(graph-list)",
        "Return graph-mode sequencer definitions with current-pattern overrides.",
        move |_args, _ctx| {
            Ok(lisp_list(
                state_for_graph_list
                    .published_sequencers()
                    .into_iter()
                    .filter_map(|published| published.graph)
                    .map(|manifest| {
                        let graph_overrides = state_for_graph_list.current_graph_overrides();
                        let overrides = graph_overrides_for_manifest(&graph_overrides, &manifest);
                        graph_manifest_to_value(&manifest, overrides)
                    })
                    .collect(),
            ))
        },
    );

    let state_for_graph_describe = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-describe",
        "(graph-describe id-or-name)",
        "Return one graph-mode sequencer definition.",
        move |args, _ctx| {
            let reference = args
                .first()
                .ok_or_else(|| "graph-describe expects graph id or name".to_string())?;
            let manifest = resolve_graph_manifest(&state_for_graph_describe, reference)?;
            let graph_overrides = state_for_graph_describe.current_graph_overrides();
            let overrides = graph_overrides_for_manifest(&graph_overrides, &manifest);
            Ok(graph_manifest_to_value(&manifest, overrides))
        },
    );

    let state_for_graph_node_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-value",
        "(graph-node-value sequencer node-index :delay)",
        "Return one resolved current-pattern graph node intrinsic value.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err(
                    "graph-node-value expects graph id/name, node index, and field".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_node_value, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "graph-node-value expects a field name".to_string())?;
            resolved_graph_node_value(&state_for_graph_node_value, &manifest, instance, &field)
        },
    );

    let state_for_graph_param_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-param-value",
        "(graph-param-value sequencer node-index :threshold)",
        "Return one resolved current-pattern graph node param value.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err(
                    "graph-param-value expects graph id/name, node index, and param".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_param_value, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let param = graph_key_string(&args[2])
                .ok_or_else(|| "graph-param-value expects a param name".to_string())?;
            resolved_graph_param_value(&state_for_graph_param_value, &manifest, instance, &param)
        },
    );

    let state_for_graph_edge_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge-value",
        "(graph-edge-value sequencer :from 0 :to 1 :weight)",
        "Return one resolved current-pattern graph edge param value.",
        move |args, _ctx| {
            if args.len() < 4 {
                return Err(
                    "graph-edge-value expects graph, from/to coordinates, and param".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge_value, &args[0])?;
            let query = parse_graph_edge_query(&manifest, &args[1..])?;
            resolved_graph_edge_value(&state_for_graph_edge_value, &manifest, query)
        },
    );

    let state_for_graph_node = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node",
        "(graph-node sequencer node-index :delay 2 :route 0 :seed-from 1)",
        "Set sparse per-pattern graph node intrinsic overrides.",
        move |args, ctx| {
            if args.len() < 2 {
                return Err("graph-node expects graph id/name and node index".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_node, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= manifest.shape.num_nodes() {
                return Err("graph-node node index out of range".to_string());
            }
            let edit = parse_graph_node_edit(&args[2..])?;
            let sequencer_name = manifest.name.clone();
            state_for_graph_node.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                let node = ensure_graph_node_intrinsic(graph, &manifest.node.name, instance);
                apply_graph_node_edit(node, edit);
                Ok(())
            })?;
            ctx.set_status(format!("updated graph '{sequencer_name}' node {instance}"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_param = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-param",
        "(graph-param sequencer node-index :threshold 0.75)",
        "Set one sparse per-pattern graph node param override.",
        move |args, ctx| {
            if args.len() != 4 {
                return Err("graph-param expects graph, node index, param, value".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_param, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= manifest.shape.num_nodes() {
                return Err("graph-param node index out of range".to_string());
            }
            let param = graph_key_string(&args[2]).ok_or("graph-param expects param name")?;
            let value = graph_number(&args[3]).ok_or("graph-param value must be numeric")?;
            let sequencer_name = manifest.name.clone();
            state_for_graph_param.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                upsert_graph_node_param(graph, &manifest.node.name, instance, &param, value);
                Ok(())
            })?;
            ctx.set_status(format!(
                "updated graph '{sequencer_name}' node {instance} param {param}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_edge = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge",
        "(graph-edge sequencer :from 0 :to 1 :weight 0.5)",
        "Set one sparse per-pattern graph edge param override.",
        move |args, ctx| {
            if args.len() < 6 {
                return Err("graph-edge expects graph, :from, :to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge, &args[0])?;
            let edit = parse_graph_edge_edit(&manifest, &args[1..])?;
            let sequencer_name = manifest.name.clone();
            let param_name = edit.param.clone();
            state_for_graph_edge.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                upsert_graph_edge_param(graph, edit);
                Ok(())
            })?;
            ctx.set_status(format!(
                "updated graph '{sequencer_name}' edge {param_name}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_bind_graph = Arc::clone(&state);
    runtime.register_native_with_docs(
        "bind-graph",
        "(bind-graph sequencer node-index :delay [options])",
        "Reactive handle to a graph node param/intrinsic, seeded with the resolved \
         current-pattern value. Numeric fields bind directly; pass an options list to \
         bind an enum field (route/resolution/quantize) as a dropdown index.",
        move |args, _ctx| {
            if args.len() < 3 {
                return Err("bind-graph expects graph, node index, and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= manifest.shape.num_nodes() {
                return Err("bind-graph node index out of range".to_string());
            }
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "bind-graph expects a field name".to_string())?;
            let value = match args.get(3) {
                Some(options) => {
                    let display = graph_node_display_value(
                        &state_for_bind_graph,
                        &manifest,
                        instance,
                        &field,
                    )?;
                    graph_option_index(options, &display)
                }
                None => {
                    graph_node_numeric_value(&state_for_bind_graph, &manifest, instance, &field)?
                }
            };
            Ok(graph_seeded_reactive_ref(
                graph_node_reactive_field(manifest.id, instance, &field),
                value,
            ))
        },
    );

    let state_for_graph_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-key",
        "(graph-key sequencer node-index :delay)",
        "Canonical GRAPH reactive field name for a node field. Use with \
         `(reactive-set \"GRAPH\" (graph-key ...) value)` to dirty a `bind-graph` handle.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err("graph-key expects graph, node index, and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_key, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "graph-key expects a field name".to_string())?;
            Ok(EValue::String(graph_node_reactive_field(
                manifest.id,
                instance,
                &field,
            )))
        },
    );

    let state_for_bind_graph_edge = Arc::clone(&state);
    runtime.register_native_with_docs(
        "bind-graph-edge",
        "(bind-graph-edge sequencer from to :weight)",
        "Reactive handle to a graph edge param (weight/dampening/delay), seeded with \
         the resolved current-pattern value.",
        move |args, _ctx| {
            if args.len() != 4 {
                return Err("bind-graph-edge expects graph, from, to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph_edge, &args[0])?;
            let from = parse_nonnegative_usize(&args[1], "from")?;
            let to = parse_nonnegative_usize(&args[2], "to")?;
            let param = graph_key_string(&args[3])
                .ok_or_else(|| "bind-graph-edge expects a param name".to_string())?;
            let edge_set = manifest
                .edge_sets
                .first()
                .ok_or_else(|| "bind-graph-edge requires an edge set".to_string())?;
            if from >= manifest.shape.num_nodes() || to >= manifest.shape.num_nodes() {
                return Err("bind-graph-edge from/to index out of range".to_string());
            }
            let query = GraphEdgeQuery {
                group: crate::graph::edge_set_group_id(edge_set),
                from,
                to,
                param: param.clone(),
            };
            let value = graph_number(&resolved_graph_edge_value(
                &state_for_bind_graph_edge,
                &manifest,
                query,
            )?)
            .ok_or_else(|| format!("bind-graph-edge param :{param} is not numeric"))?;
            Ok(graph_seeded_reactive_ref(
                graph_edge_reactive_field(manifest.id, from, to, &param),
                value,
            ))
        },
    );

    let state_for_graph_edge_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge-key",
        "(graph-edge-key sequencer from to :weight)",
        "Canonical GRAPH reactive field name for an edge param.",
        move |args, _ctx| {
            if args.len() != 4 {
                return Err("graph-edge-key expects graph, from, to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge_key, &args[0])?;
            let from = parse_nonnegative_usize(&args[1], "from")?;
            let to = parse_nonnegative_usize(&args[2], "to")?;
            let param = graph_key_string(&args[3])
                .ok_or_else(|| "graph-edge-key expects a param name".to_string())?;
            Ok(EValue::String(graph_edge_reactive_field(
                manifest.id,
                from,
                to,
                &param,
            )))
        },
    );

    let state_for_graph_config_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config-value",
        "(graph-config-value sequencer :reset-bars)",
        "Resolved sequencer-level config (:reset-bars or :max-poly), override-or-manifest.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("graph-config-value expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config_value, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config-value expects a field name".to_string())?;
            resolved_graph_config_value(&state_for_graph_config_value, &manifest, &field)
                .map(EValue::Number)
        },
    );

    let state_for_graph_config = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config",
        "(graph-config sequencer :reset-bars 4)",
        "Set a sequencer-level config override (:reset-bars in bars, or :max-poly).",
        move |args, ctx| {
            if args.len() != 3 {
                return Err("graph-config expects graph, field, value".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config expects a field name".to_string())?;
            let value = graph_number(&args[2])
                .ok_or_else(|| "graph-config value must be numeric".to_string())?;
            let sequencer_name = manifest.name.clone();
            set_graph_config_value(&state_for_graph_config, &manifest, &field, value)?;
            ctx.set_status(format!("updated graph '{sequencer_name}' config {field}"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_config_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config-key",
        "(graph-config-key sequencer :reset-bars)",
        "Canonical GRAPH reactive field name for a sequencer-level config field.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("graph-config-key expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config_key, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config-key expects a field name".to_string())?;
            Ok(EValue::String(graph_config_reactive_field(
                manifest.id,
                &field,
            )))
        },
    );

    let state_for_bind_graph_config = Arc::clone(&state);
    runtime.register_native_with_docs(
        "bind-graph-config",
        "(bind-graph-config sequencer :reset-bars)",
        "Reactive handle to a sequencer-level config field, seeded with the resolved value.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("bind-graph-config expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph_config, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "bind-graph-config expects a field name".to_string())?;
            let value =
                resolved_graph_config_value(&state_for_bind_graph_config, &manifest, &field)?;
            Ok(graph_seeded_reactive_ref(
                graph_config_reactive_field(manifest.id, &field),
                value,
            ))
        },
    );
}

pub fn register_neural_authoring_natives_with_selection(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    selected_neural_neurons: SharedSelectedNeuralNeurons,
) {
    let state_for_neural_list = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-list",
        "(neural-list)",
        "Return the current pattern's neural network definitions.",
        move |_args, _ctx| {
            Ok(lisp_list(
                state_for_neural_list
                    .current_neural_networks()
                    .iter()
                    .map(neural_network_to_value)
                    .collect(),
            ))
        },
    );

    let state_for_neural_create = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-create",
        "(neural-create :name \"name\" :neurons n [:enabled true] [:weights matrix])",
        "Create a neural network in the current pattern and return its structured description.",
        move |args, ctx| {
            let options = parse_neural_create_args(&args)?;
            let created = state_for_neural_create.edit_current_neural_networks(|networks| {
                let mut network = ProjectNeuralNetwork {
                    id: next_neural_network_id(networks),
                    name: options.name.clone(),
                    enabled: options.enabled,
                    num_neurons: options.num_neurons,
                    weights: options.weights.clone().unwrap_or_else(|| {
                        vec![vec![0.0; options.num_neurons]; options.num_neurons]
                    }),
                    neurons: vec![ProjectNeuron::default(); options.num_neurons],
                    ..ProjectNeuralNetwork::default()
                };
                normalize_project_neural_network_shape(&mut network)?;
                networks.push(network.clone());
                Ok(network)
            })?;
            ctx.set_status(format!("created neural network '{}'", created.name));
            Ok(neural_network_to_value(&created))
        },
    );

    let state_for_neural_describe = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-describe",
        "(neural-describe id-or-name)",
        "Return one neural network definition from the current pattern.",
        move |args, _ctx| {
            let reference = parse_neural_network_ref(
                args.first()
                    .ok_or_else(|| "neural-describe expects network id or name".to_string())?,
            )?;
            let networks = state_for_neural_describe.current_neural_networks();
            let idx = neural_network_index(&networks, &reference)?;
            Ok(neural_network_to_value(&networks[idx]))
        },
    );

    let state_for_neural_select = Arc::clone(&state);
    let selection_for_neural_select = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-select-neuron",
        "(neural-select-neuron id-or-name neuron-index)",
        "Select one neuron for UI authoring and return the full selected-neuron list.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err(
                    "neural-select-neuron expects network id/name and neuron index".to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let networks = state_for_neural_select.current_neural_networks();
            let network_idx = neural_network_index(&networks, &reference)?;
            let network = &networks[network_idx];
            if neuron_idx >= network.num_neurons {
                return Err("neuron index out of range".to_string());
            }
            let pattern_idx = state_for_neural_select
                .pattern
                .current_pattern
                .load(Ordering::Relaxed) as usize;
            let mut selection = selection_for_neural_select.lock().unwrap();
            selection.clear();
            selection.insert(SelectedNeuralNeuron {
                pattern_idx,
                network_id: network.id,
                neuron_idx,
            });
            ctx.set_status(format!(
                "selected neural neuron {}:{}:{}",
                pattern_idx, network.id, neuron_idx
            ));
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_clear = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-clear-selection",
        "(neural-clear-selection)",
        "Clear selected neural neurons and return the empty selected-neuron list.",
        move |_args, _ctx| {
            let mut selection = selection_for_neural_clear.lock().unwrap();
            selection.clear();
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_selected = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-selected-neurons",
        "(neural-selected-neurons)",
        "Return selected neural neurons as maps containing :pattern, :network-id, and :neuron.",
        move |_args, _ctx| {
            let selection = selection_for_neural_selected.lock().unwrap();
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_selected_predicate = Arc::clone(&selected_neural_neurons);
    let state_for_neural_selected_predicate = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-neuron-selected?",
        "(neural-neuron-selected? network-id neuron-index)",
        "Return true when a neural network neuron is selected in the current pattern.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err(
                    "neural-neuron-selected? expects network id and neuron index".to_string(),
                );
            }
            let network_id = match &args[0] {
                EValue::Number(id) if id.is_finite() && *id >= 0.0 => *id as u64,
                _ => return Err("network id must be a non-negative number".to_string()),
            };
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let pattern_idx = state_for_neural_selected_predicate
                .pattern
                .current_pattern
                .load(Ordering::Relaxed) as usize;
            let selection = selection_for_neural_selected_predicate.lock().unwrap();
            Ok(EValue::Bool(selection.contains(&SelectedNeuralNeuron {
                pattern_idx,
                network_id,
                neuron_idx,
            })))
        },
    );

    let state_for_neural_delete = Arc::clone(&state);
    let selection_for_neural_delete = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-delete",
        "(neural-delete id-or-name)",
        "Delete one neural network from the current pattern.",
        move |args, ctx| {
            let reference = parse_neural_network_ref(
                args.first()
                    .ok_or_else(|| "neural-delete expects network id or name".to_string())?,
            )?;
            let deleted = state_for_neural_delete.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                Ok(networks.remove(idx))
            })?;
            let pattern_idx = state_for_neural_delete
                .pattern
                .current_pattern
                .load(Ordering::Relaxed) as usize;
            selection_for_neural_delete
                .lock()
                .unwrap()
                .retain(|selected| {
                    selected.pattern_idx != pattern_idx || selected.network_id != deleted.id
                });
            ctx.set_status(format!("deleted neural network '{}'", deleted.name));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_neural_enable = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-enable",
        "(neural-enable id-or-name true)",
        "Enable or disable one neural network in the current pattern.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err("neural-enable expects network id/name and bool".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let enabled = parse_bool_value(&args[1], "neural-enable")?;
            let updated = state_for_neural_enable.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                networks[idx].enabled = enabled;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "neural network '{}' enabled={}",
                updated.name, updated.enabled
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_set = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-set",
        "(neural-set id-or-name :reset-bars 4 :energy-decay 0.994 :max-poly 2)",
        "Set global neural network options.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("neural-set expects network id/name".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let edits = parse_neural_set_args(&args[1..])?;
            let updated = state_for_neural_set.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                apply_neural_set_edits(&mut networks[idx], &edits)?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!("updated neural network '{}'", updated.name));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_neuron = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-neuron",
        "(neural-neuron id-or-name index :route track :resolution :16 :threshold 0.8 :delay 2 :quantize :8 :transpose 0)",
        "Set one neuron's route, clock, threshold, delay, quantize, transpose, and dampening options.",
        move |args, ctx| {
            if args.len() < 2 {
                return Err("neural-neuron expects network id/name and neuron index".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let edits = parse_neural_neuron_args(&args[2..])?;
            let track_count = state_for_neural_neuron.active_track_count();
            let updated = state_for_neural_neuron.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                if neuron_idx >= networks[idx].num_neurons {
                    return Err("neuron index out of range".to_string());
                }
                apply_neural_neuron_edits(
                    &mut networks[idx].neurons[neuron_idx],
                    &edits,
                    track_count,
                )?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {}",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_plock_instrument = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-plock-instrument",
        "(neural-plock-instrument id-or-name neuron track param value)",
        "Set a target-track instrument parameter p-lock for one neuron using a stored engine value.",
        move |args, ctx| {
            if args.len() != 5 {
                return Err(
                    "neural-plock-instrument expects network, neuron, track, param, value"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let param_idx = parse_nonnegative_usize(&args[3], "instrument param")?;
            let value = parse_value_arg(&args, 4, "instrument p-lock")?;
            let param_id = neural_instrument_param_id(
                &state_for_neural_plock_instrument,
                target_track,
                param_idx,
            )?;
            let updated = state_for_neural_plock_instrument.edit_current_neural_networks(
                |networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    upsert_neural_instrument_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        param_idx,
                        param_id,
                        value,
                    )?;
                    Ok(networks[idx].clone())
                },
            )?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {} instrument p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_plock_effect = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-plock-effect",
        "(neural-plock-effect id-or-name neuron track slot param value)",
        "Set a target-track audio effect parameter p-lock for one neuron using a stored engine value.",
        move |args, ctx| {
            if args.len() != 6 {
                return Err(
                    "neural-plock-effect expects network, neuron, track, slot, param, value"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let slot_idx = parse_nonnegative_usize(&args[3], "effect slot")?;
            let param_idx = parse_nonnegative_usize(&args[4], "effect param")?;
            let value = parse_value_arg(&args, 5, "effect p-lock")?;
            let param_id =
                neural_effect_param_id(&state_for_neural_plock_effect, target_track, slot_idx, param_idx)?;
            let updated = state_for_neural_plock_effect.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                upsert_neural_effect_plock(
                    &mut networks[idx],
                    neuron_idx,
                    target_track,
                    slot_idx,
                    param_idx,
                    param_id,
                    value,
                )?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {} effect p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_clear_instrument = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-clear-instrument-plock",
        "(neural-clear-instrument-plock id-or-name neuron track param)",
        "Clear a target-track instrument parameter p-lock from one neuron.",
        move |args, ctx| {
            if args.len() != 4 {
                return Err(
                    "neural-clear-instrument-plock expects network, neuron, track, param"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let param_idx = parse_nonnegative_usize(&args[3], "instrument param")?;
            let updated =
                state_for_neural_clear_instrument.edit_current_neural_networks(|networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    clear_neural_instrument_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        param_idx,
                    )?;
                    Ok(networks[idx].clone())
                })?;
            ctx.set_status(format!(
                "cleared neural network '{}' neuron {} instrument p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_clear_effect = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-clear-effect-plock",
        "(neural-clear-effect-plock id-or-name neuron track slot param)",
        "Clear a target-track audio effect parameter p-lock from one neuron.",
        move |args, ctx| {
            if args.len() != 5 {
                return Err(
                    "neural-clear-effect-plock expects network, neuron, track, slot, param"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let slot_idx = parse_nonnegative_usize(&args[3], "effect slot")?;
            let param_idx = parse_nonnegative_usize(&args[4], "effect param")?;
            let updated =
                state_for_neural_clear_effect.edit_current_neural_networks(|networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    clear_neural_effect_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        slot_idx,
                        param_idx,
                    )?;
                    Ok(networks[idx].clone())
                })?;
            ctx.set_status(format!(
                "cleared neural network '{}' neuron {} effect p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_weights = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-weights",
        "(neural-weights id-or-name '((0 1) (0 0)))",
        "Replace a neural network's full NxN weight matrix. Rows are from-neuron, columns are to-neuron.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err("neural-weights expects network id/name and matrix".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let updated = state_for_neural_weights.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                networks[idx].weights =
                    parse_neural_weight_matrix(&args[1], networks[idx].num_neurons)?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!("updated neural network '{}' weights", updated.name));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_weight = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-weight",
        "(neural-weight id-or-name :from 0 :to 1 :value 0.8)",
        "Set one matrix cell. Rows are from-neuron, columns are to-neuron.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("neural-weight expects network id/name".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let edit = parse_neural_weight_args(&args[1..])?;
            let updated = state_for_neural_weight.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                let n = networks[idx].num_neurons;
                if edit.from >= n || edit.to >= n {
                    return Err("neural-weight from/to index out of range".to_string());
                }
                normalize_project_neural_network_shape(&mut networks[idx])?;
                networks[idx].weights[edit.from][edit.to] = edit.value;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' weight {} -> {}",
                updated.name, edit.from, edit.to
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_reset_step = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-reset-step",
        "(neural-reset-step :track 0 :step 0 true) | (neural-reset-step track step true)",
        "Set or clear the dedicated neural reset flag for a step.",
        move |args, ctx| {
            let reset = parse_neural_reset_step_args(&args)?;
            state_for_neural_reset_step.set_neural_reset_step(
                reset.track,
                reset.step,
                reset.enabled,
            )?;
            ctx.set_status(format!(
                "track {} step {} neural-reset={}",
                reset.track, reset.step, reset.enabled
            ));
            Ok(EValue::Bool(reset.enabled))
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn register_sequencer_natives_with_accumulators(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
    accumulators: SharedRegisteredAccumulators,
    midi_fx: SharedRegisteredMidiFx,
    pending_midi_fx_params: SharedPendingMidiFxParams,
    midi_fx_state: SharedMidiFxState,
    accumulator_eval: SharedAccumulatorEvalContext,
    sequencers: SharedRegisteredSequencers,
    generator_tick: SharedGeneratorTickContext,
) {
    let current_track =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.track).unwrap_or(0);
    let current_step =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.cursor_step).unwrap_or(0);

    let _ = install_runtime_globals(runtime, &context, &metadata, &[]);

    // `def-sequencer` is a plain variadic builtin (NOT a macro): eseqlisp macros are
    // fixed-arity with no unquote-splicing, and builtins already receive variadic
    // evaluated args — so `(def-sequencer "name" :resolution :16 :tick (lambda () ...))`
    // works directly, with the :tick closure usable on whichever VM evaluates the form.
    // `__register-sequencer` is kept as the lower-level alias.
    let sequencers_for_register = Arc::clone(&sequencers);
    runtime.register_native_with_docs(
        "def-sequencer",
        "(def-sequencer name :resolution :16 :tick (lambda () (seq-emit ...)))",
        "Define a self-clocked lisp generator. The :tick closure runs once per :resolution boundary and emits events via seq-emit.",
        move |args, _ctx| register_sequencer_impl(&args, &sequencers_for_register),
    );
    let sequencers_for_register_alias = Arc::clone(&sequencers);
    runtime.register_native_with_docs(
        "__register-sequencer",
        "(__register-sequencer name :resolution :16 :tick (lambda () ...))",
        "Lower-level alias for def-sequencer.",
        move |args, _ctx| register_sequencer_impl(&args, &sequencers_for_register_alias),
    );

    let generator_tick_for_emit = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "seq-emit",
        "(seq-emit :track t :at :now :vel v :note n :dur (beats :8) :chord (list ...) :quantize :16)",
        "Emit an event from a generator :tick at a musical offset; the engine resolves timing to samples.",
        move |args, _ctx| {
            let mut guard = generator_tick_for_emit
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("seq-emit called outside a generator tick".to_string());
            };
            let event = build_seq_emit_event(&args, ctx)?;
            ctx.emitted.push(event);
            Ok(EValue::Bool(true))
        },
    );

    let generator_tick_for_tick = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-tick",
        "(gen-tick)",
        "0-based count of this generator's boundary crossings since reset.",
        move |_args, _ctx| {
            let guard = generator_tick_for_tick
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-tick called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let generator_tick_for_beat = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-beat",
        "(gen-beat)",
        "Musical position of this boundary in quarter-note beats.",
        move |_args, _ctx| {
            let guard = generator_tick_for_beat
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-beat called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.beat))
        },
    );

    let generator_tick_for_bar = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-bar",
        "(gen-bar)",
        "0-based bar index of this boundary (4 beats per bar).",
        move |_args, _ctx| {
            let guard = generator_tick_for_bar
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-bar called outside a generator tick".to_string());
            };
            Ok(EValue::Number((ctx.beat / 4.0).floor()))
        },
    );

    let generator_tick_for_phase = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-phase",
        "(gen-phase)",
        "Position within the current bar in beats (0..4).",
        move |_args, _ctx| {
            let guard = generator_tick_for_phase
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-phase called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.beat.rem_euclid(4.0)))
        },
    );

    let generator_tick_for_rand = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-rand",
        "(gen-rand)",
        "Deterministic pseudo-random float in [0,1), seeded per generator.",
        move |_args, _ctx| {
            let mut guard = generator_tick_for_rand
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("gen-rand called outside a generator tick".to_string());
            };
            ctx.random_state = ctx.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let bits = gen_splitmix64(ctx.random_state);
            Ok(EValue::Number((bits >> 11) as f64 / (1u64 << 53) as f64))
        },
    );

    let generator_tick_for_state_get = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "state-get",
        "(state-get \"key\") | (state-get \"key\" default)",
        "Read a persistent per-generator scalar state cell (0.0, or the given default, if unset).",
        move |args, _ctx| {
            let key = match args.first() {
                Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => s.clone(),
                _ => return Err("state-get expects a string key".to_string()),
            };
            let default = match args.get(1) {
                Some(EValue::Number(n)) => *n,
                _ => 0.0,
            };
            let guard = generator_tick_for_state_get
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("state-get called outside a generator tick".to_string());
            };
            Ok(EValue::Number(
                ctx.state.get(&key).copied().unwrap_or(default),
            ))
        },
    );

    let generator_tick_for_state_set = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "state-set!",
        "(state-set! \"key\" value)",
        "Write a persistent per-generator scalar state cell; returns the value.",
        move |args, _ctx| {
            let key = match args.first() {
                Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => s.clone(),
                _ => return Err("state-set! expects a string key".to_string()),
            };
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("state-set! expects (state-set! \"key\" number)".to_string());
            };
            let mut guard = generator_tick_for_state_set
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("state-set! called outside a generator tick".to_string());
            };
            ctx.state.insert(key, *value);
            Ok(EValue::Number(*value))
        },
    );

    runtime.register_native_with_docs(
        "gen-offset",
        "(gen-offset :16 n)",
        "Beats offset = n steps at a timebase, for seq-emit :at.",
        move |args, _ctx| {
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(n)) = args.get(1) else {
                return Err("gen-offset expects (gen-offset :timebase n)".to_string());
            };
            Ok(EValue::Number(
                *n * timebase.step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS),
            ))
        },
    );

    runtime.register_native_with_docs(
        "beats",
        "(beats :8)",
        "Beats in one step of a timebase, for seq-emit :dur.",
        move |args, _ctx| {
            let timebase = parse_timebase_arg(&args, 0)?;
            Ok(EValue::Number(timebase.step_beats(
                crate::generator::GENERATOR_RESOLUTION_REF_STEPS,
            )))
        },
    );

    let accumulators_for_register = Arc::clone(&accumulators);
    runtime.register_native_with_docs(
        "__register-accumulator",
        "(__register-accumulator name callback)",
        "Internal helper used by def-accumulator to register a named scheduler-side trigger mutation callback.",
        move |args, ctx| {
            let Some(name) = args.first() else {
                return Err("expected accumulator name".to_string());
            };
            let name = match name {
                EValue::String(name) => name.clone(),
                _ => return Err("expected accumulator name string".to_string()),
            };
            let Some(callback) = args.get(1) else {
                return Err("expected accumulator callback".to_string());
            };
            let callback = match callback {
                EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
                EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
                other => RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other)),
            };
            let mut registry = accumulators_for_register
                .lock()
                .map_err(|_| "failed to lock accumulator registry".to_string())?;
            if let Some(existing) = registry.iter_mut().find(|entry| entry.name == name) {
                existing.callback = callback.clone();
            } else {
                registry.push(RegisteredAccumulator {
                    name: name.clone(),
                    callback: callback.clone(),
                    params: Vec::new(),
                });
            }
            ctx.set_status(format!("registered accumulator '{name}'"));
            Ok(EValue::Bool(true))
        },
    );

    let midi_fx_for_register = Arc::clone(&midi_fx);
    let pending_params_for_register = Arc::clone(&pending_midi_fx_params);
    runtime.register_native_with_docs(
        "__register-midi-fx",
        "(__register-midi-fx name callback)",
        "Internal helper used by def-midi-fx to register a named scheduler-side MIDI FX callback.",
        move |args, ctx| {
            let Some(name) = args.first() else {
                return Err("expected MIDI FX name".to_string());
            };
            let name = match name {
                EValue::String(name) => name.clone(),
                _ => return Err("expected MIDI FX name string".to_string()),
            };
            let Some(callback) = args.get(1) else {
                return Err("expected MIDI FX callback".to_string());
            };
            let callback = match callback {
                EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
                EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
                other => {
                    RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other))
                }
            };
            let mut params = pending_params_for_register
                .lock()
                .map_err(|_| "failed to lock pending MIDI FX params".to_string())?
                .drain(..)
                .collect::<Vec<_>>();
            ensure_enabled_param(&mut params);
            let params = params
                .into_iter()
                .enumerate()
                .map(|(idx, mut param)| {
                    param.node_param_idx = idx as u32;
                    param
                })
                .collect::<Vec<_>>();
            let mut registry = midi_fx_for_register
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?;
            if let Some(existing) = registry.iter_mut().find(|entry| entry.name == name) {
                existing.callback = callback.clone();
                existing.params = params.clone();
            } else {
                registry.push(RegisteredAccumulator {
                    name: name.clone(),
                    callback: callback.clone(),
                    params: params.clone(),
                });
            }
            ctx.set_status(format!("registered MIDI FX '{name}'"));
            Ok(EValue::Bool(true))
        },
    );

    let pending_params_for_param = Arc::clone(&pending_midi_fx_params);
    runtime.register_native_with_docs(
        "midi-fx-param",
        "(midi-fx-param \"name\" :default value :min value :max value :enum \"a\" \"b\" ...)",
        "Declare a plockable parameter for the next def-midi-fx in a folder MIDI FX source.",
        move |args, _ctx| {
            let Some(name_value) = args.first() else {
                return Err("midi-fx-param expects a name".to_string());
            };
            let name = match name_value {
                EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => {
                    name.trim_start_matches('@').to_string()
                }
                _ => return Err("midi-fx-param name must be string/symbol/keyword".to_string()),
            };
            let param = parse_midi_fx_param_descriptor(&name, &args[1..])?;
            pending_params_for_param
                .lock()
                .map_err(|_| "failed to lock pending MIDI FX params".to_string())?
                .push(param);
            Ok(EValue::Bool(true))
        },
    );

    let state_for_reset_acc = Arc::clone(&state);
    let context_for_reset_acc = Arc::clone(&context);
    runtime.register_native_with_docs(
        "reset-acc",
        "(reset-acc) | (reset-acc track) | (reset-acc :all)",
        "Reset scheduler accumulator state for the current track, a specific 0-based track, or all tracks.",
        move |args, ctx| {
            if args.is_empty() {
                let track_idx = current_track(&context_for_reset_acc);
                state_for_reset_acc.request_accumulator_reset(track_idx);
                ctx.set_status(format!("reset accumulator for track {track_idx}"));
                return Ok(EValue::Bool(true));
            }
            match &args[0] {
                EValue::Keyword(name) if name == "all" => {
                    state_for_reset_acc.request_all_accumulator_resets();
                    ctx.set_status("reset accumulators for all tracks");
                    Ok(EValue::Bool(true))
                }
                EValue::Number(track) if *track >= 0.0 => {
                    let track_idx = *track as usize;
                    state_for_reset_acc.request_accumulator_reset(track_idx);
                    ctx.set_status(format!("reset accumulator for track {track_idx}"));
                    Ok(EValue::Bool(true))
                }
                _ => Err("reset-acc expects no args, a 0-based track index, or :all".to_string()),
            }
        },
    );

    let state_for_use_acc = Arc::clone(&state);
    let context_for_use_acc = Arc::clone(&context);
    let accumulators_for_use_acc = Arc::clone(&accumulators);
    runtime.register_native_with_docs(
        "seq-use-accumulator",
        "(seq-use-accumulator name) | (seq-use-accumulator track name)",
        "Assign a built-in or scratch accumulator to the current track, or to a specific 0-based track.",
        move |args, ctx| {
            let (track_idx, label) = match args.as_slice() {
                [EValue::String(label)] => (current_track(&context_for_use_acc), label.clone()),
                [EValue::Number(track), EValue::String(label)] if *track >= 0.0 => {
                    (*track as usize, label.clone())
                }
                _ => {
                    return Err(
                        "seq-use-accumulator expects a name string or track/name".to_string()
                    )
                }
            };
            if track_idx >= state_for_use_acc.active_track_count() {
                return Err("track out of range".to_string());
            }

            let mut names = crate::accumulator::ACCUMULATOR_REGISTRY
                .iter()
                .map(|def| def.name.to_string())
                .collect::<Vec<_>>();
            let builtin_count = names.len();
            names.extend(
                accumulators_for_use_acc
                    .lock()
                    .map_err(|_| "failed to lock accumulator registry".to_string())?
                    .iter()
                    .map(|entry| entry.name.clone()),
            );
            let Some(idx) = names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&label))
            else {
                return Err(format!("unknown accumulator '{label}'"));
            };

            let tp = &state_for_use_acc.pattern.track_params[track_idx];
            tp.set_accumulator_idx(idx);
            if idx < builtin_count {
                tp.set_script_accumulator_name(None);
                if let Some(def) = crate::accumulator::ACCUMULATOR_REGISTRY.get(idx) {
                    tp.set_accum_limit(def.default_limit);
                }
            } else {
                tp.set_script_accumulator_name(Some(names[idx].clone()));
            }
            state_for_use_acc.request_accumulator_reset(track_idx);
            state_for_use_acc.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} accumulator {}", names[idx]));
            Ok(EValue::String(names[idx].clone()))
        },
    );

    let state_for_use_midi_fx = Arc::clone(&state);
    let context_for_use_midi_fx = Arc::clone(&context);
    let midi_fx_for_use = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-use-midi-fx",
        "(seq-use-midi-fx name...) | (seq-use-midi-fx track name...)",
        "Assign a scratch MIDI FX chain to the current track, or to a specific 0-based track.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("seq-use-midi-fx expects at least one MIDI FX name".to_string());
            }
            let (track_idx, labels_start) = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => (*track as usize, 1),
                _ => (current_track(&context_for_use_midi_fx), 0),
            };
            if track_idx >= state_for_use_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            let mut chain = Vec::new();
            for arg in args.iter().skip(labels_start) {
                match arg {
                    EValue::String(label) => chain.push(label.clone()),
                    _ => return Err("seq-use-midi-fx expects string MIDI FX names".to_string()),
                }
            }
            if chain.is_empty() {
                return Err("seq-use-midi-fx expects at least one MIDI FX name".to_string());
            }
            let registry = midi_fx_for_use
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let names = registry
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>();
            for label in &chain {
                if !names.iter().any(|name| name.eq_ignore_ascii_case(label)) {
                    return Err(format!("unknown MIDI FX '{label}'"));
                }
            }
            state_for_use_midi_fx.pattern.track_params[track_idx].set_midi_fx_chain(chain.clone());
            for slot in &state_for_use_midi_fx.pattern.midi_fx_slots[track_idx] {
                slot.clear();
            }
            for (slot_idx, label) in chain.iter().enumerate() {
                if slot_idx >= state_for_use_midi_fx.pattern.midi_fx_slots[track_idx].len() {
                    break;
                }
                if let Some(entry) = registry
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(label))
                {
                    let desc = EffectDescriptor {
                        name: entry.name.clone(),
                        params: entry.params.clone(),
                        input_channels: 0,
                        output_channels: 0,
                        instrument_modulators: Vec::new(),
                        instrument_modulation_targets: Vec::new(),
                    };
                    state_for_use_midi_fx.pattern.midi_fx_slots[track_idx][slot_idx]
                        .sync_descriptor(&desc, 0);
                }
            }
            state_for_use_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX {:?}", chain));
            Ok(lisp_list(chain.into_iter().map(EValue::String).collect()))
        },
    );

    let state_for_clear_midi_fx = Arc::clone(&state);
    let context_for_clear_midi_fx = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-midi-fx",
        "(seq-clear-midi-fx) | (seq-clear-midi-fx track)",
        "Clear the MIDI FX chain for the current track or a specific 0-based track.",
        move |args, ctx| {
            let track_idx = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => *track as usize,
                None => current_track(&context_for_clear_midi_fx),
                _ => return Err("seq-clear-midi-fx expects no args or a track index".to_string()),
            };
            if track_idx >= state_for_clear_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            state_for_clear_midi_fx.pattern.track_params[track_idx].set_midi_fx_chain(Vec::new());
            for slot in &state_for_clear_midi_fx.pattern.midi_fx_slots[track_idx] {
                slot.clear();
            }
            state_for_clear_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX cleared"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_midi_fx_chain = Arc::clone(&state);
    let context_for_midi_fx_chain = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-midi-fx-chain",
        "(seq-midi-fx-chain) | (seq-midi-fx-chain track)",
        "Return the MIDI FX chain for the current track or a specific 0-based track.",
        move |args, _ctx| {
            let track_idx = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => *track as usize,
                None => current_track(&context_for_midi_fx_chain),
                _ => return Err("seq-midi-fx-chain expects no args or a track index".to_string()),
            };
            if track_idx >= state_for_midi_fx_chain.active_track_count() {
                return Err("track out of range".to_string());
            }
            Ok(lisp_list(
                state_for_midi_fx_chain.pattern.track_params[track_idx]
                    .midi_fx_chain()
                    .into_iter()
                    .map(EValue::String)
                    .collect(),
            ))
        },
    );

    register_neural_authoring_natives(runtime, Arc::clone(&state));

    let state_for_set_midi_fx_param = Arc::clone(&state);
    let context_for_set_midi_fx_param = Arc::clone(&context);
    let midi_fx_for_set_param = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-set-midi-fx-param",
        "(seq-set-midi-fx-param slot param value) | (seq-set-midi-fx-param track slot param value)",
        "Set a MIDI FX slot default parameter on the current track or a specific track.",
        move |args, ctx| {
            let (track_idx, idx) = match args.first() {
                Some(EValue::Number(track)) if args.len() >= 4 && *track >= 0.0 => {
                    (*track as usize, 1)
                }
                _ => (current_track(&context_for_set_midi_fx_param), 0),
            };
            if track_idx >= state_for_set_midi_fx_param.active_track_count() {
                return Err("track out of range".to_string());
            }
            let Some(EValue::Number(slot)) = args.get(idx) else {
                return Err("seq-set-midi-fx-param expects numeric slot".to_string());
            };
            let slot_idx = *slot as usize;
            let Some(param_ref) = args.get(idx + 1) else {
                return Err("seq-set-midi-fx-param expects param".to_string());
            };
            let value = parse_value_arg(&args, idx + 2, "MIDI FX param")?;
            let registry = midi_fx_for_set_param
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let param_desc = midi_fx_param_descriptor_for_slot(
                &state_for_set_midi_fx_param,
                &registry,
                track_idx,
                slot_idx,
                param_ref,
            )?;
            let param_idx = param_desc.node_param_idx as usize;
            let slot = state_for_set_midi_fx_param
                .pattern
                .midi_fx_slots
                .get(track_idx)
                .and_then(|slots| slots.get(slot_idx))
                .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
            slot.defaults.set(param_idx, param_desc.clamp(value));
            state_for_set_midi_fx_param.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {track_idx} MIDI FX slot {slot_idx} param {param_idx}"
            ));
            Ok(EValue::Number(slot.defaults.get(param_idx) as f64))
        },
    );

    let state_for_plock_midi_fx = Arc::clone(&state);
    let context_for_plock_midi_fx = Arc::clone(&context);
    let midi_fx_for_plock = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-plock-midi-fx",
        "(seq-plock-midi-fx step slot param value) | (seq-plock-midi-fx track step slot param value)",
        "Set a MIDI FX parameter p-lock on a step.",
        move |args, ctx| {
            let (track_idx, idx) = match args.first() {
                Some(EValue::Number(track)) if args.len() >= 5 && *track >= 0.0 => {
                    (*track as usize, 1)
                }
                _ => (current_track(&context_for_plock_midi_fx), 0),
            };
            if track_idx >= state_for_plock_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            let Some(EValue::Number(step)) = args.get(idx) else {
                return Err("seq-plock-midi-fx expects numeric step".to_string());
            };
            let Some(EValue::Number(slot)) = args.get(idx + 1) else {
                return Err("seq-plock-midi-fx expects numeric slot".to_string());
            };
            let step_idx = (*step as usize).min(crate::sequencer::MAX_STEPS - 1);
            let slot_idx = *slot as usize;
            let Some(param_ref) = args.get(idx + 2) else {
                return Err("seq-plock-midi-fx expects param".to_string());
            };
            let value = parse_value_arg(&args, idx + 3, "MIDI FX p-lock")?;
            let registry = midi_fx_for_plock
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let param_desc = midi_fx_param_descriptor_for_slot(
                &state_for_plock_midi_fx,
                &registry,
                track_idx,
                slot_idx,
                param_ref,
            )?;
            let param_idx = param_desc.node_param_idx as usize;
            let slot = state_for_plock_midi_fx
                .pattern
                .midi_fx_slots
                .get(track_idx)
                .and_then(|slots| slots.get(slot_idx))
                .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
            slot.set_plock(step_idx, param_idx, param_desc.clamp(value));
            state_for_plock_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {track_idx} step {step_idx} MIDI FX slot {slot_idx} param {param_idx}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_midi_fx_position = Arc::clone(&state);
    let context_for_midi_fx_position = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-midi-fx-position",
        "(seq-set-midi-fx-position :post-accumulator) | (seq-set-midi-fx-position track :post-accumulator)",
        "Set whether the track MIDI FX chain runs before or after the visible accumulator slot.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("seq-set-midi-fx-position expects a position".to_string());
            }
            let (track_idx, pos_idx) = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => (*track as usize, 1),
                _ => (current_track(&context_for_midi_fx_position), 0),
            };
            if track_idx >= state_for_midi_fx_position.active_track_count() {
                return Err("track out of range".to_string());
            }
            let position = match args.get(pos_idx) {
                Some(EValue::Keyword(name)) | Some(EValue::String(name))
                    if name == "post-accumulator" || name == "post" =>
                {
                    crate::sequencer::MidiFxPosition::PostAccumulator
                }
                Some(EValue::Keyword(name)) | Some(EValue::String(name))
                    if name == "pre-accumulator" || name == "pre" =>
                {
                    return Err(
                        "pre-accumulator MIDI FX position is not implemented yet".to_string()
                    );
                }
                _ => {
                    return Err(
                        "seq-set-midi-fx-position expects :pre-accumulator or :post-accumulator"
                            .to_string(),
                    )
                }
            };
            state_for_midi_fx_position.pattern.track_params[track_idx]
                .set_midi_fx_position(position);
            state_for_midi_fx_position.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX position {position:?}"));
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_suppress = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-suppress",
        "(acc-suppress)",
        "Suppress the source trigger for the current accumulator evaluation.",
        move |_args, _ctx| {
            let mut guard = acc_eval_for_suppress
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            eval.suppressed = true;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_chord = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-chord",
        "(acc-chord)",
        "Return the current trigger chord as a list of transpose values.",
        move |_args, _ctx| {
            let guard = acc_eval_for_chord
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            Ok(lisp_list(
                eval.chord
                    .iter()
                    .map(|note| EValue::Number(*note as f64))
                    .collect(),
            ))
        },
    );

    let acc_eval_for_chord_durations = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-chord-durations",
        "(acc-chord-durations)",
        "Return the current trigger chord note durations in source step units.",
        move |_args, _ctx| {
            let guard = acc_eval_for_chord_durations
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let durations = accumulator_chord_notes(eval)
                .into_iter()
                .map(|note| EValue::Number(note.duration_steps as f64))
                .collect();
            Ok(lisp_list(durations))
        },
    );

    let acc_eval_for_arp_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-count",
        "(acc-arp-count :16)",
        "Return the number of arp ticks needed to cover the current chord durations at a timebase.",
        move |args, _ctx| {
            let guard = acc_eval_for_arp_count
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            Ok(EValue::Number(
                accumulator_arp_count(eval, rate_beats) as f64
            ))
        },
    );

    let acc_eval_for_arp_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-note",
        "(acc-arp-note :16 tick)",
        "Return the chord note for an arp tick at a timebase, or nil once that note's duration has ended.",
        move |args, _ctx| {
            let guard = acc_eval_for_arp_note
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(tick)) = args.get(1) else {
                return Err("acc-arp-note expects numeric tick".to_string());
            };
            if *tick < 0.0 {
                return Ok(EValue::Nil);
            }
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            Ok(accumulator_arp_note(eval, rate_beats, *tick as usize)
                .map(|note| EValue::Number(note as f64))
                .unwrap_or(EValue::Nil))
        },
    );

    let acc_eval_for_arp_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-emit",
        "(acc-arp-emit :16 tick :vel 0.8 ...)",
        "Emit one duration-aware arpeggiated note for a tick. Returns false when that note lane has ended.",
        move |args, _ctx| {
            let mut guard = acc_eval_for_arp_emit
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(tick)) = args.get(1) else {
                return Err("acc-arp-emit expects numeric tick".to_string());
            };
            if *tick < 0.0 {
                return Ok(EValue::Bool(false));
            }
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            let Some(note) = accumulator_arp_note(eval, rate_beats, *tick as usize) else {
                return Ok(EValue::Bool(false));
            };

            let mut resolved = eval.resolved;
            resolved.transpose = note;
            if eval.step_beats > 0.0 {
                resolved.duration = (rate_beats / eval.step_beats).max(0.0);
            }
            let mut chord = Vec::new();
            let mut chord_durations = Vec::new();
            let target_track =
                apply_acc_emit_overrides(&args, 2, &mut resolved, &mut chord, &mut chord_durations)?;
            eval.emitted.push(EmittedAccumulatorEvent {
                offset_beats: *tick as f32 * rate_beats,
                track: target_track,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose: eval.chord_step_transpose,
                effect_params: eval.effect_params.clone(),
                instrument_params: eval.instrument_params.clone(),
            });
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-emit",
        "(acc-emit offset :vel value :note transpose ...) | (acc-emit :16 value ...)",
        "Emit a derived trigger at a musical offset. Numeric offsets use the source step's timebase; an initial timebase keyword overrides that unit.",
        move |args, _ctx| {
            let mut guard = acc_eval_for_emit
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let (offset_beats, idx) = parse_acc_emit_offset(&args, eval.step_beats, eval.num_steps)?;
            let mut resolved = eval.resolved;
            let mut chord = eval.chord.clone();
            let mut chord_durations = eval.chord_durations.clone();
            let chord_step_transpose = eval.chord_step_transpose;
            let target_track =
                apply_acc_emit_overrides(&args, idx, &mut resolved, &mut chord, &mut chord_durations)?;
            eval.emitted.push(EmittedAccumulatorEvent {
                offset_beats,
                track: target_track,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                effect_params: eval.effect_params.clone(),
                instrument_params: eval.instrument_params.clone(),
            });
            Ok(EValue::Bool(true))
        },
    );

    let fx_eval_for_suppress = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-suppress",
        "(fx-suppress)",
        "Suppress the input event for the current MIDI FX evaluation.",
        move |_args, _ctx| eval_suppress_current_event(&fx_eval_for_suppress, "MIDI FX"),
    );

    let fx_eval_for_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-emit",
        "(fx-emit offset :vel value :note transpose ...) | (fx-emit :16 value ...)",
        "Emit a derived MIDI FX event at a musical offset.",
        move |args, _ctx| eval_emit_current_event(&fx_eval_for_emit, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-count",
        "(fx-arp-count :16)",
        "Return the number of arp ticks for the current MIDI FX note spans at a timebase.",
        move |args, _ctx| eval_arp_count_current_event(&fx_eval_for_arp_count, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-note",
        "(fx-arp-note :16 tick)",
        "Return the note for an arp tick at a timebase, or nil once that note lane has ended.",
        move |args, _ctx| eval_arp_note_current_event(&fx_eval_for_arp_note, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-emit",
        "(fx-arp-emit :16 tick :vel 0.8 ...)",
        "Emit one duration-aware arpeggiated MIDI FX event for a tick.",
        move |args, _ctx| eval_arp_emit_current_event(&fx_eval_for_arp_emit, &args, "MIDI FX"),
    );

    let fx_eval_for_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-time",
        "(fx-time :16) | (fx-time :8t units)",
        "Return a duration in beats for a MIDI FX timebase and optional unit count.",
        move |args, _ctx| eval_fx_time(&fx_eval_for_time, &args),
    );

    let fx_eval_for_source_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-source-time",
        "(fx-source-time) | (fx-source-time units)",
        "Return a duration in beats for the current source step and optional unit count.",
        move |args, _ctx| eval_fx_source_time(&fx_eval_for_source_time, &args),
    );

    let fx_eval_for_phase_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-phase-time",
        "(fx-phase-time)",
        "Return the current MIDI FX scheduling phase in beats. Live quantized triggers advance this across repeated invocations.",
        move |_args, _ctx| eval_fx_phase_time(&fx_eval_for_phase_time),
    );

    let fx_eval_for_phase_tick = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-phase-tick",
        "(fx-phase-tick :16)",
        "Return the current MIDI FX scheduling phase as a tick index at the given timebase.",
        move |args, _ctx| eval_fx_phase_tick(&fx_eval_for_phase_tick, &args),
    );

    let fx_eval_for_param = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-param",
        "(fx-param \"name\") | (fx-param index)",
        "Read the current MIDI FX slot parameter value, resolving the current step's p-lock over the slot default.",
        move |args, _ctx| eval_midi_fx_param(&fx_eval_for_param, &args),
    );

    let fx_eval_for_track = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-track",
        "(fx-track)",
        "Return the zero-based source track for the current MIDI FX event.",
        move |_args, _ctx| eval_midi_fx_track(&fx_eval_for_track),
    );

    let fx_eval_for_arp_emit_directed = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-emit-directed",
        "(fx-arp-emit-directed rate tick direction :vel 0.8 ...)",
        "Emit one arpeggiated MIDI FX note with direction 0=up, 1=down, 2=up-down, 3=random.",
        move |args, _ctx| {
            eval_arp_emit_directed_current_event(&fx_eval_for_arp_emit_directed, &args, "MIDI FX")
        },
    );

    let fx_eval_for_note_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-count",
        "(fx-note-count)",
        "Return the number of notes available to the current MIDI FX event.",
        move |_args, _ctx| {
            let guard = fx_eval_for_note_count
                .lock()
                .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("MIDI FX context not active".to_string());
            };
            let count = eval
                .note_spans
                .as_ref()
                .map(|spans| spans.len())
                .unwrap_or_else(|| accumulator_chord_notes(eval).len());
            Ok(EValue::Number(count as f64))
        },
    );

    let fx_eval_for_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note",
        "(fx-note index)",
        "Return the transpose value for a note in the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note, &args, FxNoteField::Transpose),
    );

    let fx_eval_for_note_start = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-start",
        "(fx-note-start index)",
        "Return a note start time in beats relative to the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note_start, &args, FxNoteField::Start),
    );

    let fx_eval_for_note_end = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-end",
        "(fx-note-end index)",
        "Return a note end time in beats relative to the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note_end, &args, FxNoteField::End),
    );

    let fx_eval_for_notes = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-notes",
        "(fx-notes)",
        "Return all notes for the current MIDI FX event as maps with :note, :start, and :end fields.",
        move |_args, _ctx| eval_note_spans_as_list(&fx_eval_for_notes),
    );

    let fx_eval_for_state_get = Arc::clone(&accumulator_eval);
    let fx_state_for_get = Arc::clone(&midi_fx_state);
    runtime.register_native_with_docs(
        "fx-state-get",
        "(fx-state-get key) | (fx-state-get key default)",
        "Read persistent per-track/per-MIDI-FX state for the current MIDI FX callback.",
        move |args, _ctx| eval_midi_fx_state_get(&fx_eval_for_state_get, &fx_state_for_get, &args),
    );

    let fx_eval_for_state_set = Arc::clone(&accumulator_eval);
    let fx_state_for_set = Arc::clone(&midi_fx_state);
    runtime.register_native_with_docs(
        "fx-state-set",
        "(fx-state-set key value)",
        "Write persistent per-track/per-MIDI-FX state for the current MIDI FX callback.",
        move |args, _ctx| eval_midi_fx_state_set(&fx_eval_for_state_set, &fx_state_for_set, &args),
    );

    let acc_eval_for_set_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-set-step-param",
        "(acc-set-step-param :param value)",
        "Set a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let value = parse_value_arg(&args, 1, "step param")?;
            let mut guard = acc_eval_for_set_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_set(&mut eval.resolved, param, value);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-add-step-param",
        "(acc-add-step-param :param delta)",
        "Add a delta to a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let delta = parse_value_arg(&args, 1, "step param delta")?;
            let mut guard = acc_eval_for_add_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_add(&mut eval.resolved, param, delta);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_scale_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-scale-step-param",
        "(acc-scale-step-param :param factor)",
        "Scale a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let factor = parse_value_arg(&args, 1, "step param factor")?;
            let mut guard = acc_eval_for_scale_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_scale(&mut eval.resolved, param, factor);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect = Arc::clone(&metadata);
    let context_for_acc_effect = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-effect-param",
        "(acc-set-effect-param ref normalized) | (acc-set-effect-param slot param-index normalized)",
        "Set an effect parameter for the current accumulator trigger using a normalized 0.0..1.0 value.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_normalized(eval, slot_idx, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_effect = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_effect = Arc::clone(&metadata);
    let context_for_add_acc_effect = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-effect-param",
        "(acc-add-effect-param ref normalized-delta) | (acc-add-effect-param slot param-index normalized-delta)",
        "Add a normalized delta to the current resolved effect parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized_delta = parse_value_arg(&args, value_idx, "effect param delta")?;
            let mut guard = acc_eval_for_add_effect
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_effect);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_add_acc_effect,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            add_effect_param_normalized(
                eval,
                slot_idx,
                param_idx,
                normalized_delta,
                &param_desc,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_raw = Arc::clone(&metadata);
    let context_for_acc_effect_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-effect-param-raw",
        "(acc-set-effect-param-raw ref value) | (acc-set-effect-param-raw slot param-index value)",
        "Set an effect parameter for the current accumulator trigger using a raw stored value.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_raw(eval, slot_idx, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_effect_raw = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_effect_raw = Arc::clone(&metadata);
    let context_for_add_acc_effect_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-effect-param-raw",
        "(acc-add-effect-param-raw ref delta) | (acc-add-effect-param-raw slot param-index delta)",
        "Add a raw delta to the current resolved effect parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let delta = parse_value_arg(&args, value_idx, "effect param delta")?;
            let mut guard = acc_eval_for_add_effect_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_effect_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_add_acc_effect_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            add_effect_param_raw(eval, slot_idx, param_idx, delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_alias = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_alias = Arc::clone(&metadata);
    let context_for_acc_effect_alias = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-plock-effect",
        "(acc-plock-effect ref normalized) | (acc-plock-effect slot param-index normalized)",
        "Alias for acc-set-effect-param using normalized values.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_alias
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_alias);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_alias,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_normalized(eval, slot_idx, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_alias_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_alias_raw = Arc::clone(&metadata);
    let context_for_acc_effect_alias_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-plock-effect-raw",
        "(acc-plock-effect-raw ref value) | (acc-plock-effect-raw slot param-index value)",
        "Alias for acc-set-effect-param-raw.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_alias_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_alias_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_alias_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_raw(eval, slot_idx, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_instrument = Arc::clone(&accumulator_eval);
    let metadata_for_acc_instrument = Arc::clone(&metadata);
    let context_for_acc_instrument = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-instrument-param",
        "(acc-set-instrument-param ref normalized) | (acc-set-instrument-param param-index normalized)",
        "Set an instrument parameter for the current accumulator trigger using a normalized 0.0..1.0 value.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "instrument param")?;
            let mut guard = acc_eval_for_instrument
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_instrument);
            let param_desc =
                accumulator_instrument_param_desc(&metadata_for_acc_instrument, track_idx, param_idx)?;
            set_instrument_param_normalized(eval, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_instrument = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_instrument = Arc::clone(&metadata);
    let context_for_add_acc_instrument = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-instrument-param",
        "(acc-add-instrument-param ref normalized-delta) | (acc-add-instrument-param param-index normalized-delta)",
        "Add a normalized delta to the current resolved instrument parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let normalized_delta = parse_value_arg(&args, value_idx, "instrument param delta")?;
            let mut guard = acc_eval_for_add_instrument
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_instrument);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_add_acc_instrument,
                track_idx,
                param_idx,
            )?;
            add_instrument_param_normalized(eval, param_idx, normalized_delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_instrument_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_instrument_raw = Arc::clone(&metadata);
    let context_for_acc_instrument_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-instrument-param-raw",
        "(acc-set-instrument-param-raw ref value) | (acc-set-instrument-param-raw param-index value)",
        "Set an instrument parameter for the current accumulator trigger using a raw stored value.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "instrument param")?;
            let mut guard = acc_eval_for_instrument_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_instrument_raw);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_acc_instrument_raw,
                track_idx,
                param_idx,
            )?;
            set_instrument_param_raw(eval, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_instrument_raw = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_instrument_raw = Arc::clone(&metadata);
    let context_for_add_acc_instrument_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-instrument-param-raw",
        "(acc-add-instrument-param-raw ref delta) | (acc-add-instrument-param-raw param-index delta)",
        "Add a raw delta to the current resolved instrument parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let delta = parse_value_arg(&args, value_idx, "instrument param delta")?;
            let mut guard = acc_eval_for_add_instrument_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_instrument_raw);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_add_acc_instrument_raw,
                track_idx,
                param_idx,
            )?;
            add_instrument_param_raw(eval, param_idx, delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let context_for_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-current-track",
        "(seq-current-track)",
        "Return the current 0-based track index for the scratch context.",
        move |_args, _ctx| Ok(EValue::Number(current_track(&context_for_track) as f64)),
    );

    let state_for_set_track = Arc::clone(&state);
    let context_for_set_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-current-track",
        "(seq-set-current-track track)",
        "Set the current 0-based track index for subsequent scratch operations.",
        move |args, ctx| {
            let Some(EValue::Number(track)) = args.first() else {
                return Err("expected 0-based track index".to_string());
            };
            let track = *track as isize;
            if track < 0 {
                return Err("track indices must be >= 0".to_string());
            }
            let track_count = state_for_set_track.active_track_count() as isize;
            if track >= track_count {
                return Err(format!("track out of range (0..{})", track_count - 1));
            }
            let track_idx = track as usize;
            if let Ok(mut eval_ctx) = context_for_set_track.lock() {
                eval_ctx.track = track_idx;
            }
            ctx.set_status(format!("current track {}", track));
            Ok(EValue::Number(track as f64))
        },
    );

    let state_for_host_set_track = Arc::clone(&state);
    let context_for_host_set_track = Arc::clone(&context);
    runtime.register_native("__host-set-current-track", move |args, _ctx| {
        let Some(EValue::Number(track)) = args.first() else {
            return Err("expected 0-based track index".to_string());
        };
        let track = *track as isize;
        if track < 0 {
            return Err("track indices must be >= 0".to_string());
        }
        let track_count = state_for_host_set_track.active_track_count() as isize;
        if track >= track_count {
            return Err(format!("track out of range (0..{})", track_count - 1));
        }
        if let Ok(mut eval_ctx) = context_for_host_set_track.lock() {
            eval_ctx.track = track as usize;
        }
        Ok(EValue::Number(track as f64))
    });
    runtime.document_symbol(
        "__host-set-current-track",
        "(__host-set-current-track track)",
        "Internal host hook that updates the scratch evaluation context's current track.",
    );

    let context_for_step = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-current-step",
        "(seq-current-step)",
        "Return the current 0-based step index for the scratch context.",
        move |_args, _ctx| Ok(EValue::Number(current_step(&context_for_step) as f64)),
    );

    let context_for_host_set_step = Arc::clone(&context);
    runtime.register_native("__host-set-current-step", move |args, _ctx| {
        let Some(EValue::Number(step)) = args.first() else {
            return Err("expected 0-based step index".to_string());
        };
        let step = *step as isize;
        if step < 0 {
            return Err("step indices must be >= 0".to_string());
        }
        if let Ok(mut eval_ctx) = context_for_host_set_step.lock() {
            eval_ctx.cursor_step = step as usize;
        }
        Ok(EValue::Number(step as f64))
    });
    runtime.document_symbol(
        "__host-set-current-step",
        "(__host-set-current-step step)",
        "Internal host hook that updates the scratch evaluation context's current step.",
    );

    let state_for_steps = Arc::clone(&state);
    let context_for_steps = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-num-steps",
        "(seq-num-steps)",
        "Return the number of steps in the current track.",
        move |_args, _ctx| {
            let track = current_track(&context_for_steps);
            Ok(EValue::Number(
                state_for_steps.pattern.track_params[track].get_num_steps() as f64,
            ))
        },
    );

    let state_for_toggle = Arc::clone(&state);
    let context_for_toggle = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-toggle-step",
        "(seq-toggle-step step)",
        "Toggle the active state of a 0-based step in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_toggle);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_toggle.toggle_step_and_clear_plocks(track_idx, step_idx);
            let active = state_for_toggle.pattern.patterns[track_idx].is_active(step_idx);
            ctx.set_status(format!(
                "track {} step {} {}",
                track_idx,
                step_idx,
                if active { "on" } else { "off" }
            ));
            Ok(EValue::Bool(active))
        },
    );

    let state_for_step_on = Arc::clone(&state);
    let context_for_step_on = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step-on",
        "(seq-step-on step)",
        "Ensure a 0-based step is active in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_on);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_step_on.pattern.patterns[track_idx].set_step_active(step_idx, true);
            ctx.set_status(format!("track {} step {} on", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step_off = Arc::clone(&state);
    let context_for_step_off = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step-off",
        "(seq-step-off step)",
        "Ensure a 0-based step is inactive in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_off);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_step_off.clear_step_payload(track_idx, step_idx);
            ctx.set_status(format!("track {} step {} off", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_clear_step = Arc::clone(&state);
    let context_for_clear_step = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-step",
        "(seq-clear-step step)",
        "Clear all payload data for a 0-based step in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_clear_step);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_clear_step.clear_step_payload(track_idx, step_idx);
            ctx.set_status(format!("track {} step {} cleared", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_clear_track = Arc::clone(&state);
    let context_for_clear_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-track",
        "(seq-clear-track)",
        "Clear all step payloads in the current track.",
        move |_args, ctx| {
            let track_idx = current_track(&context_for_clear_track);
            let num_steps = state_for_clear_track.pattern.track_params[track_idx].get_num_steps();
            for step in 0..num_steps {
                state_for_clear_track.clear_step_payload(track_idx, step);
            }
            ctx.set_status(format!("track {} cleared", track_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_velocity = Arc::clone(&state);
    let context_for_velocity = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-velocity",
        "(seq-set-velocity step value)",
        "Set the velocity parameter for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_velocity);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected velocity value".to_string());
            };
            state_for_velocity.set_step_param(
                track_idx,
                step_idx,
                StepParam::Velocity,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} velocity {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_transpose = Arc::clone(&state);
    let context_for_transpose = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-transpose",
        "(seq-set-transpose step value)",
        "Set the transpose parameter for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_transpose);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected transpose value".to_string());
            };
            state_for_transpose.set_step_param(
                track_idx,
                step_idx,
                StepParam::Transpose,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} transpose {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_adjust = Arc::clone(&state);
    let context_for_adjust = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-adjust-transpose",
        "(seq-adjust-transpose step delta)",
        "Adjust the transpose parameter for a 0-based step by a delta.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_adjust);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected transpose delta".to_string());
            };
            state_for_adjust.adjust_step_param(
                track_idx,
                step_idx,
                StepParam::Transpose,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} transpose adjusted by {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step = Arc::clone(&state);
    let context_for_step_native = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step",
        "(seq-step step)",
        "Return a map snapshot for a 0-based step in the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_step_native);
            let step_idx = parse_step_arg(&args, 0)?;
            Ok(step_snapshot_to_value(
                step_idx,
                state_for_step.capture_step_snapshot(track_idx, step_idx),
            ))
        },
    );

    let state_for_track_steps = Arc::clone(&state);
    let context_for_track_steps = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-track-steps",
        "(seq-track-steps)",
        "Return a list of step snapshot maps for the current track.",
        move |_args, _ctx| {
            let track_idx = current_track(&context_for_track_steps);
            let num_steps = state_for_track_steps.pattern.track_params[track_idx].get_num_steps();
            let mut steps = Vec::with_capacity(num_steps);
            for step_idx in 0..num_steps {
                steps.push(step_snapshot_to_value(
                    step_idx,
                    state_for_track_steps.capture_step_snapshot(track_idx, step_idx),
                ));
            }
            Ok(lisp_list(steps))
        },
    );

    let state_for_rotate = Arc::clone(&state);
    let context_for_rotate = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-rotate-track",
        "(seq-rotate-track amount)",
        "Rotate the current track by the given step amount.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_rotate);
            let Some(EValue::Number(direction)) = args.first() else {
                return Err("expected rotation direction".to_string());
            };
            let num_steps = state_for_rotate.pattern.track_params[track_idx].get_num_steps();
            let steps: Vec<usize> = (0..num_steps).collect();
            state_for_rotate.rotate_steps(track_idx, &steps, *direction as isize);
            ctx.set_status(format!(
                "track {} rotated by {}",
                track_idx, *direction as isize
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step_plock = Arc::clone(&state);
    let context_for_step_plock = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-step",
        "(seq-plock-step step :param value)",
        "Parameter-lock a step parameter using a keyword name.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let param = parse_step_param_arg(&args, 1)?;
            let value = parse_value_arg(&args, 2, "step param")?;
            state_for_step_plock.set_step_param(track_idx, step_idx, param, value);
            ctx.set_status(format!(
                "track {} step {} {} {}",
                track_idx,
                step_idx,
                param.short_label(),
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_timebase_plock = Arc::clone(&state);
    let context_for_timebase_plock = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-timebase",
        "(seq-plock-timebase step :timebase)",
        "Set a timebase override for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_timebase_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let timebase = parse_timebase_arg(&args, 1)?;
            state_for_timebase_plock.pattern.timebase_plocks[track_idx].set(step_idx, timebase);
            state_for_timebase_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} timebase {}",
                track_idx,
                step_idx,
                timebase.label()
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_effect_plock = Arc::clone(&state);
    let context_for_effect_plock = Arc::clone(&context);
    let metadata_for_effect_plock = Arc::clone(&metadata);
    let context_for_effect_param_name = Arc::clone(&context);
    let metadata_for_effect_name = Arc::clone(&metadata);
    runtime.register_native_with_docs("seq-effect-param-name", "(seq-effect-param-name slot param-index)", "Return the parameter name for a 0-based effect slot and 0-based parameter index on the current track.", move |args, _ctx| {
        let track_idx = current_track(&context_for_effect_param_name);
        let slot_idx = parse_slot_arg(&args, 0)?;
        let param_idx = parse_param_index_arg(&args, 1)?;
        let name = metadata_for_effect_name
            .lock()
            .ok()
            .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
            .as_ref()
            .and_then(|slots| slots.get(slot_idx))
            .and_then(|desc| desc.params.get(param_idx))
            .map(|param| param.name.clone())
            .ok_or_else(|| "effect parameter out of range".to_string())?;
        Ok(EValue::String(name))
    });

    let context_for_effect_param_names = Arc::clone(&context);
    let metadata_for_effect_names = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-effect-param-names",
        "(seq-effect-param-names slot)",
        "Return a list of parameter names for a 0-based effect slot on the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_effect_param_names);
            let slot_idx = parse_slot_arg(&args, 0)?;
            let params = metadata_for_effect_names
                .lock()
                .ok()
                .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|slots| slots.get(slot_idx))
                .map(|desc| {
                    desc.params
                        .iter()
                        .map(|param| EValue::String(param.name.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| "effect slot out of range".to_string())?;
            Ok(lisp_list(params))
        },
    );

    runtime.register_native_with_docs(
        "seq-plock-effect",
        "(seq-plock-effect step ref normalized) | (seq-plock-effect step slot param-index normalized)",
        "Set an effect parameter lock for a 0-based step using a normalized 0.0..1.0 value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_effect_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 1)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect p-lock")?;
            let Some(slot) = state_for_effect_plock.pattern.effect_chains[track_idx].get(slot_idx)
            else {
                return Err("effect slot out of range".to_string());
            };
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("effect param index out of range".to_string());
            }
            let param_desc = metadata_for_effect_plock
                .lock()
                .ok()
                .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|slots| slots.get(slot_idx))
                .and_then(|desc| desc.params.get(param_idx))
                .cloned()
                .ok_or_else(|| "effect descriptor missing for parameter".to_string())?;
            let value = param_desc.denormalize(normalized);
            slot.set_plock(step_idx, param_idx, value);
            state_for_effect_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} effect {} param {} {}",
                track_idx,
                step_idx,
                slot_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_effect_plock_raw = Arc::clone(&state);
    let context_for_effect_plock_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-effect-raw",
        "(seq-plock-effect-raw step ref value) | (seq-plock-effect-raw step slot param-index value)",
        "Set an effect parameter lock for a 0-based step using the stored engine value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_effect_plock_raw);
            let step_idx = parse_step_arg(&args, 0)?;
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 1)?;
            let value = parse_value_arg(&args, value_idx, "effect p-lock")?;
            let Some(slot) =
                state_for_effect_plock_raw.pattern.effect_chains[track_idx].get(slot_idx)
            else {
                return Err("effect slot out of range".to_string());
            };
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("effect param index out of range".to_string());
            }
            slot.set_plock(step_idx, param_idx, value);
            state_for_effect_plock_raw.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} effect {} param {} {}",
                track_idx,
                step_idx,
                slot_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_instrument_plock = Arc::clone(&state);
    let context_for_instrument_plock = Arc::clone(&context);
    let metadata_for_instrument_plock = Arc::clone(&metadata);
    let context_for_instrument_param_name = Arc::clone(&context);
    let metadata_for_instrument_name = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-instrument-param-name",
        "(seq-instrument-param-name param-index)",
        "Return the parameter name for a 0-based instrument parameter index on the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_instrument_param_name);
            let param_idx = parse_param_index_arg(&args, 0)?;
            let name = metadata_for_instrument_name
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|desc| desc.params.get(param_idx))
                .map(|param| param.name.clone())
                .ok_or_else(|| "instrument parameter out of range".to_string())?;
            Ok(EValue::String(name))
        },
    );

    let context_for_instrument_param_names = Arc::clone(&context);
    let metadata_for_instrument_names = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-instrument-param-names",
        "(seq-instrument-param-names)",
        "Return a list of parameter names for the current track's instrument.",
        move |_args, _ctx| {
            let track_idx = current_track(&context_for_instrument_param_names);
            let params = metadata_for_instrument_names
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .map(|desc| {
                    desc.params
                        .iter()
                        .filter(|param| !param.name.eq_ignore_ascii_case("enabled"))
                        .map(|param| EValue::String(param.name.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| "instrument descriptor missing".to_string())?;
            Ok(lisp_list(params))
        },
    );

    runtime.register_native_with_docs(
        "seq-plock-instrument",
        "(seq-plock-instrument step ref normalized) | (seq-plock-instrument step param-index normalized)",
        "Set an instrument parameter lock for a 0-based step using a normalized 0.0..1.0 value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_instrument_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 1)?;
            let normalized = parse_normalized_arg(&args, value_idx, "instrument p-lock")?;
            let slot = &state_for_instrument_plock.pattern.instrument_slots[track_idx];
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("instrument param index out of range".to_string());
            }
            let param_desc = metadata_for_instrument_plock
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|desc| desc.params.get(param_idx))
                .cloned()
                .ok_or_else(|| "instrument descriptor missing for parameter".to_string())?;
            let value = param_desc.denormalize(normalized);
            slot.set_plock(step_idx, param_idx, value);
            state_for_instrument_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} instrument param {} {}",
                track_idx,
                step_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_instrument_plock_raw = Arc::clone(&state);
    let context_for_instrument_plock_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-instrument-raw",
        "(seq-plock-instrument-raw step ref value) | (seq-plock-instrument-raw step param-index value)",
        "Set an instrument parameter lock for a 0-based step using the stored engine value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_instrument_plock_raw);
            let step_idx = parse_step_arg(&args, 0)?;
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 1)?;
            let value = parse_value_arg(&args, value_idx, "instrument p-lock")?;
            let slot = &state_for_instrument_plock_raw.pattern.instrument_slots[track_idx];
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("instrument param index out of range".to_string());
            }
            slot.set_plock(step_idx, param_idx, value);
            state_for_instrument_plock_raw.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} instrument param {} {}",
                track_idx,
                step_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );
}

fn fallback_effect_descriptors(track_count: usize) -> Vec<Vec<EffectDescriptor>> {
    (0..track_count)
        .map(|_| EffectDescriptor::default_full_chain())
        .collect()
}

fn fallback_instrument_descriptors(track_count: usize) -> Vec<EffectDescriptor> {
    (0..track_count)
        .map(|_| {
            let mut desc = EffectDescriptor::builtin_delay();
            for (idx, param) in desc.params.iter_mut().enumerate() {
                param.node_param_idx = idx as u32;
            }
            desc
        })
        .collect()
}

fn shared_native_metadata(
    effect_descriptors: Vec<Vec<EffectDescriptor>>,
    instrument_descriptors: Vec<EffectDescriptor>,
) -> SharedSequencerNativeMetadata {
    Arc::new(Mutex::new(SequencerNativeMetadata {
        effect_descriptors,
        instrument_descriptors,
    }))
}

fn install_runtime_globals(
    runtime: &mut Runtime,
    context: &SharedSequencerEvalContext,
    metadata: &SharedSequencerNativeMetadata,
    previous_globals: &[String],
) -> Vec<String> {
    for name in previous_globals {
        runtime.set_global_value(name, EValue::Nil);
    }

    let track = context.lock().map(|ctx| ctx.track).unwrap_or(0);
    let (effect_descriptors, instrument_descriptor) = metadata
        .lock()
        .ok()
        .map(|metadata| {
            (
                metadata
                    .effect_descriptors
                    .get(track)
                    .cloned()
                    .unwrap_or_default(),
                metadata.instrument_descriptors.get(track).cloned(),
            )
        })
        .unwrap_or_default();

    let mut installed = Vec::new();
    for (slot_idx, desc) in effect_descriptors.iter().enumerate() {
        let global_name = sanitize_symbol_name(&desc.name, true);
        if global_name.is_empty() {
            continue;
        }
        let mut fields: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
        for (param_idx, param) in desc.params.iter().enumerate() {
            let field_name = sanitize_symbol_name(&param.name, false);
            if field_name.is_empty() {
                continue;
            }
            fields.insert(
                field_name,
                lisp_value(lisp_list(vec![
                    EValue::Number(slot_idx as f64),
                    EValue::Number(param_idx as f64),
                ])),
            );
        }
        runtime.set_global_value(&global_name, EValue::Map(fields));
        installed.push(global_name);
    }

    if let Some(desc) = instrument_descriptor {
        let global_name = sanitize_symbol_name(&desc.name, true);
        if !global_name.is_empty() {
            let mut fields: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
            for (param_idx, param) in desc.params.iter().enumerate() {
                let field_name = sanitize_symbol_name(&param.name, false);
                if field_name.is_empty() {
                    continue;
                }
                fields.insert(
                    field_name,
                    lisp_value(lisp_list(vec![EValue::Number(param_idx as f64)])),
                );
            }
            runtime.set_global_value(&global_name, EValue::Map(fields));
            installed.push(global_name);
        }
    }

    installed
}

fn parse_step_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(step)) = args.get(idx) else {
        return Err("expected 0-based step index".to_string());
    };
    if *step < 0.0 {
        return Err("step index must be >= 0".to_string());
    }
    Ok(*step as usize)
}

fn parse_slot_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(slot)) = args.get(idx) else {
        return Err("expected 0-based slot index".to_string());
    };
    if *slot < 0.0 {
        return Err("slot index must be >= 0".to_string());
    }
    Ok(*slot as usize)
}

fn parse_param_index_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(param_idx)) = args.get(idx) else {
        return Err("expected 0-based parameter index".to_string());
    };
    if *param_idx < 0.0 {
        return Err("parameter index must be >= 0".to_string());
    }
    Ok(*param_idx as usize)
}

fn parse_effect_param_ref_arg(value: &EValue) -> Result<(usize, usize), String> {
    let EValue::List(items) = value else {
        return Err("expected effect param ref or slot/param indices".to_string());
    };
    if items.len() != 2 {
        return Err("effect param ref must be a 2-item list".to_string());
    }
    let slot_idx = match &*items[0].borrow() {
        EValue::Number(slot_idx) if *slot_idx >= 0.0 => *slot_idx as usize,
        _ => return Err("effect param ref slot index must be >= 0".to_string()),
    };
    let param_idx = match &*items[1].borrow() {
        EValue::Number(param_idx) if *param_idx >= 0.0 => *param_idx as usize,
        _ => return Err("effect param ref param index must be >= 0".to_string()),
    };
    Ok((slot_idx, param_idx))
}

fn parse_effect_param_target_arg(
    args: &[EValue],
    idx: usize,
) -> Result<(usize, usize, usize), String> {
    if let Some(value) = args.get(idx) {
        if matches!(value, EValue::List(_)) {
            let (slot_idx, param_idx) = parse_effect_param_ref_arg(value)?;
            return Ok((slot_idx, param_idx, idx + 1));
        }
    }
    Ok((
        parse_slot_arg(args, idx)?,
        parse_param_index_arg(args, idx + 1)?,
        idx + 2,
    ))
}

fn parse_instrument_param_ref_arg(value: &EValue) -> Result<usize, String> {
    let EValue::List(items) = value else {
        return Err("expected instrument param ref or parameter index".to_string());
    };
    if items.len() != 1 {
        return Err("instrument param ref must be a 1-item list".to_string());
    }
    match &*items[0].borrow() {
        EValue::Number(param_idx) if *param_idx >= 0.0 => Ok(*param_idx as usize),
        _ => Err("instrument param ref index must be >= 0".to_string()),
    }
}

fn parse_instrument_param_target_arg(
    args: &[EValue],
    idx: usize,
) -> Result<(usize, usize), String> {
    if let Some(value) = args.get(idx) {
        if matches!(value, EValue::List(_)) {
            return Ok((parse_instrument_param_ref_arg(value)?, idx + 1));
        }
    }
    Ok((parse_param_index_arg(args, idx)?, idx + 1))
}

fn parse_value_arg(args: &[EValue], idx: usize, label: &str) -> Result<f32, String> {
    let Some(EValue::Number(value)) = args.get(idx) else {
        return Err(format!("expected {label} value"));
    };
    Ok(*value as f32)
}

fn parse_normalized_arg(args: &[EValue], idx: usize, label: &str) -> Result<f32, String> {
    Ok(parse_value_arg(args, idx, label)?.clamp(0.0, 1.0))
}

fn acc_emit_number(value: &EValue, label: &str) -> Result<f32, String> {
    match value {
        EValue::Number(value) => Ok(*value as f32),
        _ => Err(format!("acc-emit expected numeric {label}")),
    }
}

fn apply_acc_emit_overrides(
    args: &[EValue],
    mut idx: usize,
    resolved: &mut ResolvedStep,
    chord: &mut Vec<f32>,
    chord_durations: &mut Vec<f32>,
) -> Result<Option<usize>, String> {
    let mut target_track = None;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(name) | EValue::String(name) | EValue::Symbol(name) => {
                name.to_ascii_lowercase()
            }
            _ => return Err("acc-emit expects keyword/value override pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("acc-emit missing value for :{key}"));
        };
        match key.as_str() {
            "vel" | "velocity" => {
                resolved.velocity = acc_emit_number(value, "velocity")?.clamp(0.0, 1.0);
            }
            "transpose" | "trn" => {
                resolved.transpose = acc_emit_number(value, "transpose")?;
            }
            "note" => {
                resolved.transpose = acc_emit_number(value, "note")?;
                chord.clear();
                chord_durations.clear();
            }
            "duration" | "dur" => {
                resolved.duration = acc_emit_number(value, "duration")?.max(0.0);
            }
            "speed" | "spd" => {
                resolved.speed = acc_emit_number(value, "speed")?.max(0.0);
            }
            "pan" => {
                resolved.pan = acc_emit_number(value, "pan")?.clamp(-1.0, 1.0);
            }
            "chop" | "chp" => {
                resolved.chop = acc_emit_number(value, "chop")?.max(1.0);
            }
            "track" => {
                let track = acc_emit_number(value, "track")?;
                if track < 0.0 {
                    return Err("acc-emit :track must be >= 0".to_string());
                }
                target_track = Some(track as usize);
            }
            _ => return Err(format!("acc-emit unknown override :{key}")),
        }
        idx += 1;
    }
    Ok(target_track)
}

fn eval_suppress_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    eval.suppressed = true;
    Ok(EValue::Bool(true))
}

fn eval_emit_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let (offset_beats, idx) = parse_acc_emit_offset(args, eval.step_beats, eval.num_steps)?;
    let mut resolved = eval.resolved;
    let mut chord = eval.chord.clone();
    let mut chord_durations = eval.chord_durations.clone();
    let chord_step_transpose = eval.chord_step_transpose;
    let target_track =
        apply_acc_emit_overrides(args, idx, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

fn eval_arp_count_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_ref() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    Ok(EValue::Number(
        accumulator_arp_count(eval, rate_beats) as f64
    ))
}

fn eval_arp_note_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_ref() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("arp note helper expects numeric tick".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Nil);
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    Ok(accumulator_arp_note(eval, rate_beats, *tick as usize)
        .map(|note| EValue::Number(note as f64))
        .unwrap_or(EValue::Nil))
}

fn eval_arp_emit_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("arp emit helper expects numeric tick".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Bool(false));
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    let Some(note) = accumulator_arp_note(eval, rate_beats, *tick as usize) else {
        return Ok(EValue::Bool(false));
    };

    let mut resolved = eval.resolved;
    resolved.transpose = note;
    if eval.step_beats > 0.0 {
        resolved.duration = (rate_beats / eval.step_beats).max(0.0);
    }
    let mut chord = Vec::new();
    let mut chord_durations = Vec::new();
    let target_track =
        apply_acc_emit_overrides(args, 2, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats: *tick as f32 * rate_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose: eval.chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

fn eval_arp_emit_directed_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("directed arp emit expects numeric tick".to_string());
    };
    let Some(EValue::Number(direction)) = args.get(2) else {
        return Err("directed arp emit expects numeric direction".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Bool(false));
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    let Some(note) =
        accumulator_arp_note_directed(eval, rate_beats, *tick as usize, *direction as i32)
    else {
        return Ok(EValue::Bool(false));
    };

    let mut resolved = eval.resolved;
    resolved.transpose = note;
    if eval.step_beats > 0.0 {
        resolved.duration = (rate_beats / eval.step_beats).max(0.0);
    }
    let mut chord = Vec::new();
    let mut chord_durations = Vec::new();
    let target_track =
        apply_acc_emit_overrides(args, 3, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats: *tick as f32 * rate_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose: eval.chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

fn eval_fx_time(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let units = match args.get(1) {
        Some(EValue::Number(units)) => *units as f32,
        None => 1.0,
        _ => return Err("fx-time units must be numeric".to_string()),
    };
    Ok(EValue::Number(
        (timebase.step_beats(eval.num_steps).max(0.0) as f32 * units) as f64,
    ))
}

fn eval_fx_source_time(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let units = match args.first() {
        Some(EValue::Number(units)) => *units as f32,
        None => 1.0,
        _ => return Err("fx-source-time units must be numeric".to_string()),
    };
    Ok(EValue::Number((eval.step_beats * units) as f64))
}

fn eval_fx_phase_time(accumulator_eval: &SharedAccumulatorEvalContext) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    Ok(EValue::Number(eval.arp_phase_beats.max(0.0) as f64))
}

fn eval_fx_phase_tick(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    if rate_beats <= 0.0 {
        return Ok(EValue::Number(0.0));
    }
    Ok(EValue::Number(
        (eval.arp_phase_beats.max(0.0) / rate_beats).floor() as f64,
    ))
}

fn parse_midi_fx_param_ref(eval: &AccumulatorEvalContext, value: &EValue) -> Result<usize, String> {
    match value {
        EValue::Number(index) if *index >= 0.0 => Ok(*index as usize),
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => eval
            .midi_fx_param_names
            .iter()
            .position(|param| param.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown MIDI FX param '{name}'")),
        _ => Err("MIDI FX param ref must be a name or index".to_string()),
    }
}

fn eval_midi_fx_param(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(param_ref) = args.first() else {
        return Err("fx-param expects a name or index".to_string());
    };
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let param_idx = parse_midi_fx_param_ref(eval, param_ref)?;
    if param_idx >= eval.midi_fx_slot.num_params as usize {
        return Err("MIDI FX param index out of range".to_string());
    }
    let value = eval
        .midi_fx_slot
        .plocks
        .get(eval.step_index)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
        .unwrap_or_else(|| {
            eval.midi_fx_slot
                .defaults
                .get(param_idx)
                .copied()
                .unwrap_or(0.0)
        });
    Ok(EValue::Number(value as f64))
}

fn eval_midi_fx_track(accumulator_eval: &SharedAccumulatorEvalContext) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let Some((track, _)) = eval.midi_fx_scope.as_ref() else {
        return Err("fx-track is only available inside def-midi-fx".to_string());
    };
    Ok(EValue::Number(*track as f64))
}

enum FxNoteField {
    Transpose,
    Start,
    End,
}

fn eval_note_span_field(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    field: FxNoteField,
) -> Result<EValue, String> {
    let Some(EValue::Number(index)) = args.first() else {
        return Err("note helper expects numeric index".to_string());
    };
    if *index < 0.0 {
        return Ok(EValue::Nil);
    }
    let index = *index as usize;
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    if let Some(spans) = eval.note_spans.as_ref() {
        let Some(span) = spans.get(index) else {
            return Ok(EValue::Nil);
        };
        return Ok(EValue::Number(match field {
            FxNoteField::Transpose => span.transpose as f64,
            FxNoteField::Start => span.start_beats as f64,
            FxNoteField::End => span.end_beats as f64,
        }));
    }
    let notes = accumulator_chord_notes(eval);
    let Some(note) = notes.get(index) else {
        return Ok(EValue::Nil);
    };
    Ok(EValue::Number(match field {
        FxNoteField::Transpose => note.transpose as f64,
        FxNoteField::Start => 0.0,
        FxNoteField::End => (note.duration_steps * eval.step_beats) as f64,
    }))
}

fn eval_note_spans_as_list(
    accumulator_eval: &SharedAccumulatorEvalContext,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let notes = if let Some(spans) = eval.note_spans.as_ref() {
        spans
            .iter()
            .map(|span| (span.transpose, span.start_beats, span.end_beats))
            .collect::<Vec<_>>()
    } else {
        accumulator_chord_notes(eval)
            .into_iter()
            .map(|note| (note.transpose, 0.0, note.duration_steps * eval.step_beats))
            .collect::<Vec<_>>()
    };
    Ok(lisp_list(
        notes
            .into_iter()
            .map(|(transpose, start_beats, end_beats)| {
                let mut map = HashMap::new();
                map.insert("note".to_string(), lisp_number(transpose as f64));
                map.insert("start".to_string(), lisp_number(start_beats as f64));
                map.insert("end".to_string(), lisp_number(end_beats as f64));
                EValue::Map(map)
            })
            .collect(),
    ))
}

fn midi_fx_state_user_key(value: &EValue) -> Result<String, String> {
    match value {
        EValue::String(key) | EValue::Keyword(key) => Ok(key.clone()),
        _ => Err("MIDI FX state key must be a string or keyword".to_string()),
    }
}

fn current_midi_fx_state_key(
    accumulator_eval: &SharedAccumulatorEvalContext,
    user_key: &str,
) -> Result<String, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let Some((track, fx_name)) = eval.midi_fx_scope.as_ref() else {
        return Err("MIDI FX state is only available inside def-midi-fx".to_string());
    };
    Ok(format!("{track}\u{0}{fx_name}\u{0}{user_key}"))
}

fn eval_midi_fx_state_get(
    accumulator_eval: &SharedAccumulatorEvalContext,
    midi_fx_state: &SharedMidiFxState,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(key_value) = args.first() else {
        return Err("fx-state-get expects a key".to_string());
    };
    let user_key = midi_fx_state_user_key(key_value)?;
    let key = current_midi_fx_state_key(accumulator_eval, &user_key)?;
    Ok(midi_fx_state
        .lock()
        .map_err(|_| "failed to lock MIDI FX state".to_string())?
        .get(&key)
        .cloned()
        .unwrap_or_else(|| args.get(1).cloned().unwrap_or(EValue::Nil)))
}

fn eval_midi_fx_state_set(
    accumulator_eval: &SharedAccumulatorEvalContext,
    midi_fx_state: &SharedMidiFxState,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(key_value) = args.first() else {
        return Err("fx-state-set expects a key and value".to_string());
    };
    let Some(value) = args.get(1).cloned() else {
        return Err("fx-state-set expects a value".to_string());
    };
    let user_key = midi_fx_state_user_key(key_value)?;
    let key = current_midi_fx_state_key(accumulator_eval, &user_key)?;
    midi_fx_state
        .lock()
        .map_err(|_| "failed to lock MIDI FX state".to_string())?
        .insert(key, value.clone());
    Ok(value)
}

#[derive(Clone, Copy)]
struct AccumulatorChordNote {
    transpose: f32,
    duration_steps: f32,
}

fn accumulator_chord_notes(eval: &AccumulatorEvalContext) -> Vec<AccumulatorChordNote> {
    if eval.chord.is_empty() {
        return vec![AccumulatorChordNote {
            transpose: eval.resolved.transpose,
            duration_steps: eval.resolved.duration.max(0.0),
        }];
    }
    eval.chord
        .iter()
        .enumerate()
        .map(|(idx, note)| AccumulatorChordNote {
            transpose: *note,
            duration_steps: eval
                .chord_durations
                .get(idx)
                .copied()
                .filter(|duration| *duration > 0.0)
                .unwrap_or(eval.resolved.duration)
                .max(0.0),
        })
        .collect()
}

fn accumulator_arp_count(eval: &AccumulatorEvalContext, rate_beats: f32) -> usize {
    if rate_beats <= 0.0 {
        return 0;
    }
    if let Some(note_spans) = eval.note_spans.as_ref() {
        let max_end = note_spans
            .iter()
            .map(|note| note.end_beats)
            .fold(0.0_f32, f32::max);
        return (max_end / rate_beats).ceil().max(0.0) as usize;
    }
    let notes = accumulator_chord_notes(eval);
    if notes.is_empty() || eval.step_beats <= 0.0 {
        return 0;
    }
    let max_duration_beats = notes
        .iter()
        .map(|note| note.duration_steps * eval.step_beats)
        .fold(0.0_f32, f32::max);
    (max_duration_beats / rate_beats).ceil().max(0.0) as usize
}

fn accumulator_arp_note(
    eval: &AccumulatorEvalContext,
    rate_beats: f32,
    tick: usize,
) -> Option<f32> {
    accumulator_arp_note_directed(eval, rate_beats, tick, 0)
}

fn directed_note_index(tick: usize, len: usize, direction: i32) -> usize {
    if len <= 1 {
        return 0;
    }
    match direction {
        1 => len - 1 - (tick % len),
        2 => {
            let period = len * 2 - 2;
            let pos = tick % period;
            if pos < len {
                pos
            } else {
                period - pos
            }
        }
        3 => {
            let mut x = tick as u64;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 32) as usize) % len
        }
        _ => tick % len,
    }
}

fn accumulator_arp_note_directed(
    eval: &AccumulatorEvalContext,
    rate_beats: f32,
    tick: usize,
    direction: i32,
) -> Option<f32> {
    if rate_beats <= 0.0 {
        return None;
    }
    let phase_tick = (eval.arp_phase_beats.max(0.0) / rate_beats).floor() as usize;
    let elapsed = tick as f32 * rate_beats;
    if let Some(note_spans) = eval.note_spans.as_ref() {
        let phased_tick = tick.saturating_add(phase_tick);
        let active = note_spans
            .iter()
            .filter(|note| {
                elapsed >= note.start_beats - f32::EPSILON
                    && elapsed < note.end_beats - f32::EPSILON
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return None;
        }
        return Some(active[directed_note_index(phased_tick, active.len(), direction)].transpose);
    }
    let notes = accumulator_chord_notes(eval);
    if notes.is_empty() || eval.step_beats <= 0.0 {
        return None;
    }
    let note_idx = directed_note_index(tick.saturating_add(phase_tick), notes.len(), direction);
    let duration_beats = notes[note_idx].duration_steps * eval.step_beats;
    if elapsed < duration_beats - f32::EPSILON {
        Some(notes[note_idx].transpose)
    } else {
        None
    }
}

fn parse_acc_emit_offset(
    args: &[EValue],
    default_step_beats: f32,
    num_steps: usize,
) -> Result<(f32, usize), String> {
    let Some(first) = args.first() else {
        return Err("acc-emit expects an offset".to_string());
    };
    match first {
        EValue::Number(offset) => Ok((*offset as f32 * default_step_beats, 1)),
        EValue::Keyword(_) | EValue::String(_) => {
            if matches!(first, EValue::Keyword(name) | EValue::String(name) if name == "beats") {
                let Some(EValue::Number(offset)) = args.get(1) else {
                    return Err("acc-emit :beats expects numeric offset".to_string());
                };
                return Ok((*offset as f32, 2));
            }
            let timebase = parse_timebase_arg(args, 0)?;
            let Some(EValue::Number(offset)) = args.get(1) else {
                return Err("acc-emit explicit timebase expects numeric offset".to_string());
            };
            Ok((
                *offset as f32 * timebase.step_beats(num_steps).max(0.0) as f32,
                2,
            ))
        }
        _ => Err("acc-emit expects numeric offset or timebase keyword".to_string()),
    }
}

fn apply_step_param_set(resolved: &mut ResolvedStep, param: StepParam, value: f32) {
    match param {
        StepParam::Duration => resolved.duration = value.max(0.0),
        StepParam::Velocity => resolved.velocity = value.clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = value.max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose = value,
        StepParam::Pan => resolved.pan = value.clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = value.max(1.0),
    }
}

fn apply_step_param_add(resolved: &mut ResolvedStep, param: StepParam, delta: f32) {
    match param {
        StepParam::Duration => resolved.duration = (resolved.duration + delta).max(0.0),
        StepParam::Velocity => resolved.velocity = (resolved.velocity + delta).clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = (resolved.speed + delta).max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose += delta,
        StepParam::Pan => resolved.pan = (resolved.pan + delta).clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = (resolved.chop + delta).max(1.0),
    }
}

fn apply_step_param_scale(resolved: &mut ResolvedStep, param: StepParam, factor: f32) {
    match param {
        StepParam::Duration => resolved.duration = (resolved.duration * factor).max(0.0),
        StepParam::Velocity => resolved.velocity = (resolved.velocity * factor).clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = (resolved.speed * factor).max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose *= factor,
        StepParam::Pan => resolved.pan = (resolved.pan * factor).clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = (resolved.chop * factor).max(1.0),
    }
}

fn accumulator_effect_param_desc(
    metadata: &SharedSequencerNativeMetadata,
    track_idx: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<EffectDescriptorParamSnapshot, String> {
    metadata
        .lock()
        .ok()
        .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
        .as_ref()
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx))
        .cloned()
        .map(EffectDescriptorParamSnapshot::from)
        .ok_or_else(|| "effect descriptor missing for parameter".to_string())
}

fn accumulator_instrument_param_desc(
    metadata: &SharedSequencerNativeMetadata,
    track_idx: usize,
    param_idx: usize,
) -> Result<EffectDescriptorParamSnapshot, String> {
    metadata
        .lock()
        .ok()
        .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
        .as_ref()
        .and_then(|desc| desc.params.get(param_idx))
        .cloned()
        .map(EffectDescriptorParamSnapshot::from)
        .ok_or_else(|| "instrument descriptor missing for parameter".to_string())
}

#[derive(Clone)]
struct EffectDescriptorParamSnapshot {
    min: f32,
    max: f32,
    default: f32,
    kind: crate::effects::ParamKind,
    scaling: crate::effects::ParamScaling,
}

impl From<crate::effects::ParamDescriptor> for EffectDescriptorParamSnapshot {
    fn from(value: crate::effects::ParamDescriptor) -> Self {
        Self {
            min: value.min,
            max: value.max,
            default: value.default,
            kind: value.kind,
            scaling: value.scaling,
        }
    }
}

impl EffectDescriptorParamSnapshot {
    fn clamp(&self, value: f32) -> f32 {
        value.clamp(self.min, self.max)
    }

    fn normalize(&self, value: f32) -> f32 {
        let value = self.clamp(value);
        let range = self.max - self.min;
        if range <= 0.0 {
            return 0.0;
        }
        match self.scaling {
            crate::effects::ParamScaling::Linear => ((value - self.min) / range).clamp(0.0, 1.0),
            crate::effects::ParamScaling::Exponential => {
                if self.min <= 0.0 || self.max <= 0.0 {
                    ((value - self.min) / range).clamp(0.0, 1.0)
                } else {
                    let log_min = self.min.ln();
                    let log_max = self.max.ln();
                    let log_range = log_max - log_min;
                    if log_range <= 0.0 {
                        0.0
                    } else {
                        ((value.max(self.min).ln() - log_min) / log_range).clamp(0.0, 1.0)
                    }
                }
            }
        }
    }

    fn denormalize(&self, normalized: f32) -> f32 {
        let normalized = normalized.clamp(0.0, 1.0);
        match &self.kind {
            crate::effects::ParamKind::Boolean => {
                if normalized >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            crate::effects::ParamKind::Enum { .. } => {
                let range = self.max - self.min;
                if range <= 0.0 {
                    self.min
                } else {
                    (self.min + normalized * range)
                        .round()
                        .clamp(self.min, self.max)
                }
            }
            crate::effects::ParamKind::Continuous { .. } => match self.scaling {
                crate::effects::ParamScaling::Linear => {
                    self.min + normalized * (self.max - self.min)
                }
                crate::effects::ParamScaling::Exponential => {
                    if self.min <= 0.0 || self.max <= 0.0 {
                        self.min + normalized * (self.max - self.min)
                    } else {
                        let log_min = self.min.ln();
                        let log_max = self.max.ln();
                        (log_min + normalized * (log_max - log_min)).exp()
                    }
                }
            },
        }
    }
}

fn effect_param_ids(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
) -> Result<(u64, u64), String> {
    let slot = eval
        .effect_slots
        .get(slot_idx)
        .ok_or_else(|| "effect slot out of range".to_string())?;
    if slot.node_id == 0 {
        return Err("effect slot is empty".to_string());
    }
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("effect param index out of range".to_string());
    }
    let idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32) as u64;
    if idx == u32::MAX as u64 {
        return Err("effect param index unresolved".to_string());
    }
    Ok((slot.node_id as u64, idx))
}

fn current_effect_param_raw(
    eval: &AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<f32, String> {
    let slot = eval
        .effect_slots
        .get(slot_idx)
        .ok_or_else(|| "effect slot out of range".to_string())?;
    if slot.node_id == 0 {
        return Err("effect slot is empty".to_string());
    }
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("effect param index out of range".to_string());
    }
    Ok(eval
        .effect_params
        .iter()
        .find(|param| {
            param.logical_id == slot.node_id as u64
                && param.idx
                    == slot
                        .param_node_indices
                        .get(param_idx)
                        .copied()
                        .unwrap_or(param_idx as u32) as u64
        })
        .map(|param| param.value)
        .unwrap_or(desc.default))
}

fn set_effect_param_raw(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let (logical_id, idx) = effect_param_ids(eval, slot_idx, param_idx)?;
    let value = desc.clamp(value);
    if let Some(existing) = eval
        .effect_params
        .iter_mut()
        .find(|param| param.logical_id == logical_id && param.idx == idx)
    {
        existing.value = value;
    } else {
        eval.effect_params.push(ScheduledEffectParam {
            logical_id,
            idx,
            value,
        });
    }
    Ok(())
}

fn set_effect_param_normalized(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    normalized: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    set_effect_param_raw(
        eval,
        slot_idx,
        param_idx,
        desc.denormalize(normalized),
        desc,
    )
}

fn add_effect_param_raw(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_effect_param_raw(eval, slot_idx, param_idx, desc)?;
    set_effect_param_raw(eval, slot_idx, param_idx, current + delta, desc)
}

fn add_effect_param_normalized(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    normalized_delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_effect_param_raw(eval, slot_idx, param_idx, desc)?;
    let next = (desc.normalize(current) + normalized_delta).clamp(0.0, 1.0);
    set_effect_param_normalized(eval, slot_idx, param_idx, next, desc)
}

fn instrument_param_target_and_idx(
    slot: &EffectSlotSnapshot,
    param_idx: usize,
) -> Result<(ScheduledInstrumentParamTarget, u64, u32), String> {
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("instrument param index out of range".to_string());
    }
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    let span = slot
        .param_node_spans
        .get(param_idx)
        .copied()
        .unwrap_or(1)
        .max(1);
    let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
        )
    } else {
        (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
    };
    Ok((target, idx, span))
}

fn current_instrument_param_raw(
    eval: &AccumulatorEvalContext,
    param_idx: usize,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<f32, String> {
    let (target, idx, _) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
    Ok(eval
        .instrument_params
        .iter()
        .find(|param| param.target == target && param.idx == idx)
        .map(|param| param.value)
        .unwrap_or(desc.default))
}

fn set_instrument_param_raw(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    value: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let (target, idx, span) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
    let value = desc.clamp(value);
    if let Some(existing) = eval
        .instrument_params
        .iter_mut()
        .find(|param| param.target == target && param.idx == idx)
    {
        existing.span = span;
        existing.value = value;
    } else {
        eval.instrument_params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    Ok(())
}

fn set_instrument_param_normalized(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    normalized: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    set_instrument_param_raw(eval, param_idx, desc.denormalize(normalized), desc)
}

fn add_instrument_param_raw(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_instrument_param_raw(eval, param_idx, desc)?;
    set_instrument_param_raw(eval, param_idx, current + delta, desc)
}

fn add_instrument_param_normalized(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    normalized_delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_instrument_param_raw(eval, param_idx, desc)?;
    let next = (desc.normalize(current) + normalized_delta).clamp(0.0, 1.0);
    set_instrument_param_normalized(eval, param_idx, next, desc)
}

fn parse_step_param_arg(args: &[EValue], idx: usize) -> Result<StepParam, String> {
    let Some(value) = args.get(idx) else {
        return Err("expected step param".to_string());
    };
    match value {
        EValue::Keyword(name) | EValue::String(name) | EValue::Symbol(name) => {
            let normalized = name.to_ascii_lowercase();
            match normalized.as_str() {
                "duration" | "dur" => Ok(StepParam::Duration),
                "velocity" | "vel" => Ok(StepParam::Velocity),
                "speed" | "spd" => Ok(StepParam::Speed),
                "auxa" | "aux-a" | "aux_a" | "axa" => Ok(StepParam::AuxA),
                "auxb" | "aux-b" | "aux_b" | "axb" => Ok(StepParam::AuxB),
                "transpose" | "trn" => Ok(StepParam::Transpose),
                "pan" => Ok(StepParam::Pan),
                "chop" | "chp" => Ok(StepParam::Chop),
                "sync" | "syn" => Ok(StepParam::Sync),
                "delay" | "dly" => Ok(StepParam::Delay),
                _ => Err("unknown step param".to_string()),
            }
        }
        _ => Err("expected step param keyword/string".to_string()),
    }
}

fn register_sequencer_impl(
    args: &[EValue],
    sequencers: &SharedRegisteredSequencers,
) -> Result<EValue, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    let mut resolution = Timebase::Sixteenth;
    let mut tick: Option<EValue> = None;
    let mut idx = 1;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("def-sequencer expects keyword/value pairs".to_string()),
        };
        idx += 1;
        if args.get(idx).is_none() {
            return Err(format!("def-sequencer missing value for :{key}"));
        }
        match key.as_str() {
            "resolution" | "res" => resolution = parse_timebase_arg(args, idx)?,
            "tick" => tick = Some(args[idx].clone()),
            "init" => { /* reserved for future one-time init */ }
            _ => return Err(format!("def-sequencer unknown key :{key}")),
        }
        idx += 1;
    }
    let Some(tick) = tick else {
        return Err("def-sequencer requires :tick".to_string());
    };
    // `def-sequencer` auto-quotes :tick, so it arrives as list *data* — store it as
    // re-evaluable source (run once per boundary). The low-level `__register-sequencer`
    // does not auto-quote, so its :tick arrives as a closure.
    let tick = match tick {
        EValue::List(_) => {
            RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(&tick))
        }
        closure => RegisteredAccumulatorCallback::Closure(closure),
    };
    let id = stable_sequencer_id(&name);
    let entry = RegisteredSequencer {
        id,
        name,
        resolution,
        tick,
    };
    let mut registry = sequencers
        .lock()
        .map_err(|_| "failed to lock sequencer registry".to_string())?;
    if let Some(existing) = registry.iter_mut().find(|e| e.id == entry.id) {
        *existing = entry;
    } else {
        registry.push(entry);
    }
    Ok(EValue::Number(id as f64))
}

/// Serialize an auto-quoted `:tick` body (list data) back to re-evaluable lisp
/// source, for shipping a UI-authored `def-sequencer` to the scheduler VM.
pub fn sequencer_tick_source(value: &EValue) -> String {
    eseqlisp::vm::format_lisp_source(value)
}

/// Parse a `def-sequencer` `:resolution` value (timebase keyword/number) to its
/// `Timebase` index, defaulting to sixteenth.
pub fn sequencer_resolution_index(value: &EValue) -> u8 {
    parse_timebase_arg(std::slice::from_ref(value), 0).unwrap_or(Timebase::Sixteenth) as u8
}

pub fn stable_sequencer_id(name: &str) -> u64 {
    // FNV-1a over the name; stable across processes so hot-reload matches by id.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    if hash == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        hash
    }
}

fn gen_splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

// ───────────────────────── Graph-mode manifest parsing ─────────────────────────
//
// A graph-mode `def-sequencer` arrives (after the compiler's whole-body auto-quote)
// as a flat sequence of `EValue`s: the name, keyword/value config pairs, and the
// `def-node` / `edges` sub-forms as list data. These parse that data into a
// [`crate::graph::GraphManifest`]. Graph mode is selected by the presence of a
// `def-node` sub-form (its absence keeps the existing tick-mode path).

use crate::graph::{
    EdgeSetSpec, EventSelect, GraphManifest, LeakSpec, NodeProto, ParamSpec, Reduce as GraphReduce,
    SeedFrom, ShapeSpec, StateSpec, Topology,
};

/// Clone a list value's items out of their cells, or `None` if not a list.
fn graph_list_items(value: &EValue) -> Option<Vec<EValue>> {
    match value {
        EValue::List(items) => Some(items.iter().map(|i| i.borrow().clone()).collect()),
        _ => None,
    }
}

/// The lowercased head symbol of a sub-form (`def-node`, `edges`, `grid`, …).
fn graph_head_symbol(items: &[EValue]) -> Option<String> {
    match items.first() {
        Some(EValue::Symbol(s)) => Some(s.trim_start_matches('@').to_ascii_lowercase()),
        _ => None,
    }
}

/// Normalize a keyword/symbol/string to a bare lowercase key (no leading `:`/`@`).
fn graph_keyword(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(k) | EValue::Symbol(k) | EValue::String(k) => Some(
            k.trim_start_matches('@')
                .trim_start_matches(':')
                .to_ascii_lowercase(),
        ),
        _ => None,
    }
}

fn graph_number(value: &EValue) -> Option<f64> {
    match value {
        EValue::Number(n) => Some(*n),
        _ => None,
    }
}

fn graph_symbol_string(value: &EValue) -> String {
    match value {
        EValue::Symbol(s) | EValue::String(s) | EValue::Keyword(s) => s.clone(),
        _ => String::new(),
    }
}

fn graph_timebase(value: &EValue) -> Result<Timebase, String> {
    parse_timebase_arg(std::slice::from_ref(value), 0)
}

fn graph_reduce(value: &EValue) -> GraphReduce {
    match graph_keyword(value).as_deref() {
        Some("max") => GraphReduce::Max,
        Some("min") => GraphReduce::Min,
        Some("product") => GraphReduce::Product,
        Some("count") => GraphReduce::Count,
        _ => GraphReduce::Sum,
    }
}

/// `:event` payload-selection policy on `def-node` (Layer A). Unknown/absent keeps the
/// historical last-writer-wins (`:newest`).
fn graph_event_select(value: &EValue) -> EventSelect {
    match graph_keyword(value).as_deref() {
        Some("loudest") => EventSelect::Loudest,
        Some("seed-priority") | Some("seed") => EventSelect::SeedPriority,
        Some("strongest") => EventSelect::Strongest,
        _ => EventSelect::Newest,
    }
}

/// `:off`/`none`/`nil` → no quantize; else a timebase.
fn graph_quantize(value: &EValue) -> Result<Option<Timebase>, String> {
    match graph_keyword(value).as_deref() {
        Some("off") | Some("none") | Some("nil") | Some("false") => Ok(None),
        _ => Ok(Some(graph_timebase(value)?)),
    }
}

/// `:route`/`none` → follow route; a number → single track; a list → track set.
fn graph_seed_from(value: &EValue) -> SeedFrom {
    match graph_keyword(value).as_deref() {
        Some("route") | Some("none") | Some("nil") => return SeedFrom::Route,
        _ => {}
    }
    match value {
        EValue::Number(n) if *n >= 0.0 => SeedFrom::Tracks(vec![*n as usize]),
        EValue::List(_) => {
            let tracks = graph_list_items(value)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| graph_number(v).filter(|n| *n >= 0.0).map(|n| n as usize))
                .collect();
            SeedFrom::Tracks(tracks)
        }
        _ => SeedFrom::Route,
    }
}

fn graph_route(value: &EValue) -> Option<usize> {
    match value {
        EValue::Number(n) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    }
}

/// `(bars 4)` → beats (assumes 4/4), `(beats 2)` → beats, bare number → beats.
fn graph_bars_or_beats(value: &EValue) -> f64 {
    if let Some(items) = graph_list_items(value) {
        let n = items.get(1).and_then(graph_number).unwrap_or(0.0);
        match graph_head_symbol(&items).as_deref() {
            Some("bars") | Some("bar") => return n * 4.0,
            Some("beats") | Some("beat") => return n,
            _ => {}
        }
    }
    graph_number(value).unwrap_or(0.0)
}

/// `(name :float min max :default d)` / `(name :int min max :default d)`.
fn graph_parse_param(items: &[EValue]) -> Option<ParamSpec> {
    let name = match items.first()? {
        EValue::Symbol(s) | EValue::String(s) => s.clone(),
        _ => return None,
    };
    let mut is_int = false;
    let mut nums: Vec<f64> = Vec::new();
    let mut default: Option<f64> = None;
    let mut i = 1;
    while i < items.len() {
        match &items[i] {
            EValue::Keyword(k) => match k.trim_start_matches(':').to_ascii_lowercase().as_str() {
                "int" => is_int = true,
                "float" => is_int = false,
                "default" => {
                    i += 1;
                    default = items.get(i).and_then(graph_number);
                }
                _ => {}
            },
            EValue::Number(n) => nums.push(*n),
            _ => {}
        }
        i += 1;
    }
    let min = nums.first().copied().unwrap_or(0.0);
    let max = nums.get(1).copied().unwrap_or(0.0);
    Some(ParamSpec {
        name,
        min,
        max,
        default: default.unwrap_or(min),
        is_int,
    })
}

fn graph_parse_param_list(value: &EValue) -> Vec<ParamSpec> {
    graph_list_items(value)
        .unwrap_or_default()
        .iter()
        .filter_map(graph_list_items)
        .filter_map(|items| graph_parse_param(&items))
        .collect()
}

/// `(per-step :energy-decay)` or `(per-step 0.9)`.
fn graph_parse_leak(value: &EValue) -> Option<LeakSpec> {
    let items = graph_list_items(value)?;
    if graph_head_symbol(&items).as_deref() != Some("per-step") {
        return None;
    }
    match items.get(1) {
        Some(EValue::Number(n)) => Some(LeakSpec::PerStep(*n)),
        Some(v) if graph_keyword(v).as_deref() == Some("energy-decay") => {
            Some(LeakSpec::PerStepEnergyDecay)
        }
        _ => None,
    }
}

/// `(energy :leak (per-step :energy-decay))`.
fn graph_parse_state(items: &[EValue]) -> Option<StateSpec> {
    let name = match items.first()? {
        EValue::Symbol(s) | EValue::String(s) => s.clone(),
        _ => return None,
    };
    let mut leak = None;
    let mut i = 1;
    while i < items.len() {
        if graph_keyword(&items[i]).as_deref() == Some("leak") {
            i += 1;
            if let Some(v) = items.get(i) {
                leak = graph_parse_leak(v);
            }
        }
        i += 1;
    }
    Some(StateSpec { name, leak })
}

fn graph_parse_state_list(value: &EValue) -> Vec<StateSpec> {
    graph_list_items(value)
        .unwrap_or_default()
        .iter()
        .filter_map(graph_list_items)
        .filter_map(|items| graph_parse_state(&items))
        .collect()
}

fn graph_parse_shape(value: &EValue) -> Result<ShapeSpec, String> {
    let items =
        graph_list_items(value).ok_or_else(|| ":shape expects a generator form".to_string())?;
    let n = |idx: usize| {
        items
            .get(idx)
            .and_then(graph_number)
            .map(|n| n.max(0.0) as usize)
    };
    match graph_head_symbol(&items).as_deref() {
        Some("grid") => Ok(ShapeSpec::Grid {
            rows: n(1).ok_or("(grid R C) expects rows")?,
            cols: n(2).ok_or("(grid R C) expects cols")?,
        }),
        Some("line") => Ok(ShapeSpec::Line(n(1).ok_or("(line N) expects N")?)),
        Some("ring") => Ok(ShapeSpec::Ring(n(1).ok_or("(ring N) expects N")?)),
        other => Err(format!("unknown :shape generator: {other:?}")),
    }
}

fn graph_parse_topology(value: &EValue) -> Result<Topology, String> {
    let items =
        graph_list_items(value).ok_or_else(|| ":topology expects a generator form".to_string())?;
    match graph_head_symbol(&items).as_deref() {
        Some("all-to-all") => Ok(Topology::AllToAll),
        Some(other) => Err(format!(
            "unsupported :topology `{other}` (v1 supports all-to-all)"
        )),
        None => Err(":topology expects a generator form".to_string()),
    }
}

fn graph_parse_node_proto(items: &[EValue]) -> Result<NodeProto, String> {
    let name = match items.get(1) {
        Some(EValue::Symbol(s) | EValue::String(s)) => s.clone(),
        _ => return Err("def-node expects a name".to_string()),
    };
    let mut proto = NodeProto {
        name,
        ..NodeProto::default()
    };
    let mut i = 2;
    while i < items.len() {
        let Some(key) = graph_keyword(&items[i]) else {
            return Err("def-node expects keyword/value pairs".to_string());
        };
        i += 1;
        let Some(value) = items.get(i) else {
            return Err(format!("def-node missing value for :{key}"));
        };
        match key.as_str() {
            "resolution" | "res" => proto.resolution = graph_timebase(value)?,
            "delay" | "delay-steps" => {
                proto.delay_steps = graph_number(value).unwrap_or(0.0).max(0.0) as u32
            }
            "quantize" | "q" => proto.quantize = graph_quantize(value)?,
            "route" => proto.route = graph_route(value),
            "seed-from" => proto.seed_from = graph_seed_from(value),
            "reduce" => proto.reduce = graph_reduce(value),
            "event" | "event-select" => proto.event_select = graph_event_select(value),
            "params" => proto.params = graph_parse_param_list(value),
            "state" => proto.state = graph_parse_state_list(value),
            "update" => proto.update_source = Some(eseqlisp::vm::format_lisp_source(value)),
            _ => return Err(format!("def-node unknown key :{key}")),
        }
        i += 1;
    }
    Ok(proto)
}

fn graph_parse_edge_set(items: &[EValue]) -> Result<EdgeSetSpec, String> {
    let mut set = EdgeSetSpec {
        from: String::new(),
        to: String::new(),
        topology: Topology::AllToAll,
        gather_source: None,
        params: Vec::new(),
    };
    let mut i = 1;
    while i < items.len() {
        let Some(key) = graph_keyword(&items[i]) else {
            return Err("edges expects keyword/value pairs".to_string());
        };
        i += 1;
        let Some(value) = items.get(i) else {
            return Err(format!("edges missing value for :{key}"));
        };
        match key.as_str() {
            "from" => set.from = graph_symbol_string(value),
            "to" => set.to = graph_symbol_string(value),
            "topology" => set.topology = graph_parse_topology(value)?,
            "gather" => set.gather_source = Some(eseqlisp::vm::format_lisp_source(value)),
            "params" => set.params = graph_parse_param_list(value),
            _ => return Err(format!("edges unknown key :{key}")),
        }
        i += 1;
    }
    Ok(set)
}

/// True if these `def-sequencer` args carry a `def-node` sub-form (graph mode).
pub fn graph_mode_present(args: &[EValue]) -> bool {
    args.iter().any(|arg| {
        graph_list_items(arg)
            .map(|items| graph_head_symbol(&items).as_deref() == Some("def-node"))
            .unwrap_or(false)
    })
}

/// Parse a graph-mode `def-sequencer` arg list (including the leading name) into a
/// [`GraphManifest`].
pub fn parse_graph_manifest(args: &[EValue]) -> Result<GraphManifest, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    let id = stable_sequencer_id(&name);
    let mut shape: Option<ShapeSpec> = None;
    let mut energy_decay = 0.9;
    let mut reset_every_beats = 0.0;
    let mut seed_on_reset = 0.0;
    let mut max_poly = 0u32;
    let mut max_poly_selection = NeuralMaxPolySelection::Deterministic;
    let mut node: Option<NodeProto> = None;
    let mut edge_sets: Vec<EdgeSetSpec> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        // `def-node` / `edges` sub-forms (positional list data).
        if let Some(items) = graph_list_items(&args[i]) {
            match graph_head_symbol(&items).as_deref() {
                Some("def-node") => {
                    if node.is_some() {
                        return Err(
                            "graph-mode def-sequencer allows one def-node in v1".to_string()
                        );
                    }
                    node = Some(graph_parse_node_proto(&items)?);
                    i += 1;
                    continue;
                }
                Some("edges") => {
                    edge_sets.push(graph_parse_edge_set(&items)?);
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        // keyword/value config.
        let Some(key) = graph_keyword(&args[i]) else {
            return Err(format!(
                "graph-mode def-sequencer: unexpected form at position {i}"
            ));
        };
        i += 1;
        let Some(value) = args.get(i) else {
            return Err(format!("def-sequencer missing value for :{key}"));
        };
        match key.as_str() {
            "shape" => shape = Some(graph_parse_shape(value)?),
            "energy-decay" => energy_decay = graph_number(value).unwrap_or(0.9),
            "reset-every" => reset_every_beats = graph_bars_or_beats(value),
            "seed-on-reset" => seed_on_reset = graph_number(value).unwrap_or(0.0),
            "max-poly" => max_poly = graph_number(value).unwrap_or(0.0).max(0.0) as u32,
            "max-poly-selection" => max_poly_selection = parse_neural_max_poly_selection(value)?,
            // Resolution is per-node, not sequencer-level.
            "resolution" | "res" => {}
            _ => return Err(format!("graph-mode def-sequencer unknown key :{key}")),
        }
        i += 1;
    }

    let shape = shape.ok_or_else(|| "graph-mode def-sequencer requires :shape".to_string())?;
    let node = node.ok_or_else(|| "graph-mode def-sequencer requires a def-node".to_string())?;
    Ok(GraphManifest {
        id,
        name,
        shape,
        energy_decay,
        reset_every_beats,
        seed_on_reset,
        max_poly,
        max_poly_selection,
        node,
        edge_sets,
    })
}

fn build_seq_emit_event(
    args: &[EValue],
    ctx: &GeneratorTickContext,
) -> Result<EmittedAccumulatorEvent, String> {
    let mut resolved = crate::generator::default_resolved();
    let mut chord: Vec<f32> = Vec::new();
    let mut offset_beats: f32 = 0.0;
    let mut target_track: Option<usize> = None;
    let mut quantize: Option<Timebase> = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("seq-emit expects keyword/value pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("seq-emit missing value for :{key}"));
        };
        match key.as_str() {
            "at" => {
                offset_beats = match value {
                    EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k)
                        if k.eq_ignore_ascii_case("now") =>
                    {
                        0.0
                    }
                    EValue::Number(n) => *n as f32,
                    _ => return Err("seq-emit :at expects :now or a beats number".to_string()),
                };
            }
            "vel" | "velocity" => {
                resolved.velocity = acc_emit_number(value, "velocity")?.clamp(0.0, 1.0)
            }
            "note" | "transpose" | "trn" => resolved.transpose = acc_emit_number(value, "note")?,
            "dur" | "duration" => resolved.duration = acc_emit_number(value, "duration")?.max(0.0),
            "speed" | "spd" => resolved.speed = acc_emit_number(value, "speed")?.max(0.0),
            "pan" => resolved.pan = acc_emit_number(value, "pan")?.clamp(-1.0, 1.0),
            "chop" | "chp" => resolved.chop = acc_emit_number(value, "chop")?.max(1.0),
            "track" => {
                let track = acc_emit_number(value, "track")?;
                if track < 0.0 {
                    return Err("seq-emit :track must be >= 0".to_string());
                }
                target_track = Some(track as usize);
            }
            "chord" => {
                chord.clear();
                let EValue::List(items) = value else {
                    return Err("seq-emit :chord expects a list of transposes".to_string());
                };
                for item in items {
                    if let EValue::Number(n) = &*item.borrow() {
                        chord.push(*n as f32);
                    }
                }
            }
            "quantize" | "q" => {
                quantize = match value {
                    EValue::Bool(false) | EValue::Nil => None,
                    EValue::Keyword(k) | EValue::String(k) if k.eq_ignore_ascii_case("off") => None,
                    _ => Some(parse_timebase_arg(args, idx)?),
                };
            }
            _ => return Err(format!("seq-emit unknown key :{key}")),
        }
        idx += 1;
    }
    if let Some(grid) = quantize {
        let grid_beats = grid
            .step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS)
            .max(1e-9);
        let target = ctx.beat + offset_beats as f64;
        let position = target / grid_beats;
        let nearest = position.round();
        let snapped_units = if (position - nearest).abs() <= 1e-9 {
            nearest
        } else {
            position.ceil()
        };
        let snapped = (snapped_units * grid_beats).max(target);
        offset_beats = (snapped - ctx.beat) as f32;
    }
    Ok(EmittedAccumulatorEvent {
        offset_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations: Vec::new(),
        chord_step_transpose: 0.0,
        effect_params: Vec::new(),
        instrument_params: Vec::new(),
    })
}

fn parse_timebase_arg(args: &[EValue], idx: usize) -> Result<Timebase, String> {
    let Some(value) = args.get(idx) else {
        return Err("expected timebase".to_string());
    };
    match value {
        EValue::Number(n) if *n >= 0.0 => {
            let idx = *n as usize;
            Timebase::ALL
                .get(idx)
                .copied()
                .ok_or_else(|| "invalid timebase index".to_string())
        }
        EValue::Keyword(name) | EValue::String(name) => {
            let normalized = name.to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "whole" => Ok(Timebase::Whole),
                "2" | "half" => Ok(Timebase::Half),
                "4" | "quarter" => Ok(Timebase::Quarter),
                "8" | "eighth" => Ok(Timebase::Eighth),
                "16" | "sixteenth" => Ok(Timebase::Sixteenth),
                "32" | "thirtysecond" | "thirty-second" => Ok(Timebase::ThirtySecond),
                "64" | "sixtyfourth" | "sixty-fourth" => Ok(Timebase::SixtyFourth),
                "2t" | "halftriplet" | "half-triplet" => Ok(Timebase::HalfTriplet),
                "4t" | "quartertriplet" | "quarter-triplet" => Ok(Timebase::QuarterTriplet),
                "8t" | "eighthtriplet" | "eighth-triplet" => Ok(Timebase::EighthTriplet),
                "16t" | "sixteenthtriplet" | "sixteenth-triplet" => Ok(Timebase::SixteenthTriplet),
                "32t" | "thirtysecondtriplet" | "thirty-second-triplet" => {
                    Ok(Timebase::ThirtySecondTriplet)
                }
                "64t" | "sixtyfourthtriplet" | "sixty-fourth-triplet" => {
                    Ok(Timebase::SixtyFourthTriplet)
                }
                "prh" | "polyrhythm" => Ok(Timebase::Polyrhythm),
                _ => Err("unknown timebase".to_string()),
            }
        }
        _ => Err("expected timebase keyword/string/index".to_string()),
    }
}

fn midi_fx_attr_name(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(name) => Some(
            name.trim_start_matches('@')
                .trim_start_matches(':')
                .to_ascii_lowercase(),
        ),
        EValue::Symbol(name) | EValue::String(name)
            if name.starts_with('@') || name.starts_with(':') =>
        {
            Some(
                name.trim_start_matches('@')
                    .trim_start_matches(':')
                    .to_ascii_lowercase(),
            )
        }
        _ => None,
    }
}

fn midi_fx_attr_number(args: &[EValue], idx: usize, attr: &str) -> Result<f32, String> {
    match args.get(idx) {
        Some(EValue::Number(value)) => Ok(*value as f32),
        _ => Err(format!("midi-fx-param :{attr} expects a number")),
    }
}

fn parse_midi_fx_param_descriptor(
    name: &str,
    args: &[EValue],
) -> Result<crate::effects::ParamDescriptor, String> {
    let mut default = 0.0_f32;
    let mut min = 0.0_f32;
    let mut max = 1.0_f32;
    let mut unit = None;
    let mut labels: Option<Vec<String>> = None;
    let mut idx = 0;
    while idx < args.len() {
        let Some(attr) = midi_fx_attr_name(&args[idx]) else {
            return Err("midi-fx-param expects keyword attributes".to_string());
        };
        idx += 1;
        match attr.as_str() {
            "default" => {
                default = midi_fx_attr_number(args, idx, "default")?;
                idx += 1;
            }
            "min" => {
                min = midi_fx_attr_number(args, idx, "min")?;
                idx += 1;
            }
            "max" => {
                max = midi_fx_attr_number(args, idx, "max")?;
                idx += 1;
            }
            "unit" => {
                unit = match args.get(idx) {
                    Some(EValue::String(value))
                    | Some(EValue::Keyword(value))
                    | Some(EValue::Symbol(value)) => Some(value.clone()),
                    _ => return Err("midi-fx-param :unit expects string/symbol".to_string()),
                };
                idx += 1;
            }
            "enum" => {
                let mut enum_labels = Vec::new();
                while idx < args.len() && midi_fx_attr_name(&args[idx]).is_none() {
                    match &args[idx] {
                        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                            enum_labels.push(value.clone())
                        }
                        _ => return Err("midi-fx-param :enum labels must be strings".to_string()),
                    }
                    idx += 1;
                }
                if enum_labels.is_empty() {
                    return Err("midi-fx-param :enum expects at least one label".to_string());
                }
                max = (enum_labels.len().saturating_sub(1)) as f32;
                labels = Some(enum_labels);
            }
            other => return Err(format!("midi-fx-param unknown attribute :{other}")),
        }
    }
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }
    default = default.clamp(min, max);
    Ok(crate::effects::ParamDescriptor {
        name: name.to_string(),
        min,
        max,
        default,
        kind: labels
            .map(|labels| crate::effects::ParamKind::Enum { labels })
            .unwrap_or(crate::effects::ParamKind::Continuous { unit }),
        scaling: crate::effects::ParamScaling::Linear,
        node_param_idx: 0,
        node_param_span: 1,
        host_control: None,
        ui_metadata: None,
    })
}

fn midi_fx_param_descriptor_for_slot(
    state: &crate::sequencer::SequencerState,
    registry: &[RegisteredAccumulator],
    track_idx: usize,
    slot_idx: usize,
    param_ref: &EValue,
) -> Result<crate::effects::ParamDescriptor, String> {
    let chain = state.pattern.track_params[track_idx].midi_fx_chain();
    let fx_name = chain
        .get(slot_idx)
        .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
    let entry = registry
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(fx_name))
        .ok_or_else(|| format!("unknown MIDI FX '{fx_name}'"))?;
    match param_ref {
        EValue::Number(index) if *index >= 0.0 => entry
            .params
            .get(*index as usize)
            .cloned()
            .ok_or_else(|| "MIDI FX param index out of range".to_string()),
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => entry
            .params
            .iter()
            .find(|param| param.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| format!("unknown MIDI FX param '{name}'")),
        _ => Err("MIDI FX param must be name or index".to_string()),
    }
}

#[derive(Clone, Debug)]
enum NeuralNetworkRef {
    Id(u64),
    Name(String),
}

#[derive(Clone, Debug)]
struct NeuralCreateOptions {
    name: String,
    num_neurons: usize,
    enabled: bool,
    weights: Option<Vec<Vec<f32>>>,
}

#[derive(Clone, Debug, Default)]
struct NeuralSetEdits {
    name: Option<String>,
    reset_interval_bars: Option<f32>,
    energy_decay: Option<f32>,
    max_poly: Option<u32>,
    max_poly_selection: Option<NeuralMaxPolySelection>,
}

#[derive(Clone, Debug, Default)]
struct NeuralNeuronEdits {
    route: Option<Option<usize>>,
    resolution: Option<Timebase>,
    threshold: Option<f32>,
    delay_steps: Option<u32>,
    quantize: Option<Option<Timebase>>,
    transpose: Option<f32>,
    dampening_amount: Option<f32>,
    dampening_recovery: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
struct NeuralWeightEdit {
    from: usize,
    to: usize,
    value: f32,
}

#[derive(Clone, Copy, Debug)]
struct NeuralResetStepEdit {
    track: usize,
    step: usize,
    enabled: bool,
}

fn parse_neural_network_ref(value: &EValue) -> Result<NeuralNetworkRef, String> {
    match value {
        EValue::Number(id) if id.is_finite() && *id >= 0.0 && id.fract() == 0.0 => {
            Ok(NeuralNetworkRef::Id(*id as u64))
        }
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                Err("neural network name cannot be empty".to_string())
            } else {
                Ok(NeuralNetworkRef::Name(name))
            }
        }
        _ => Err("expected neural network id or name".to_string()),
    }
}

fn neural_network_index(
    networks: &[ProjectNeuralNetwork],
    reference: &NeuralNetworkRef,
) -> Result<usize, String> {
    match reference {
        NeuralNetworkRef::Id(id) => networks
            .iter()
            .position(|network| network.id == *id)
            .ok_or_else(|| format!("unknown neural network id {id}")),
        NeuralNetworkRef::Name(name) => {
            let matches = networks
                .iter()
                .enumerate()
                .filter(|(_, network)| network.name.eq_ignore_ascii_case(name))
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [idx] => Ok(*idx),
                [] => Err(format!("unknown neural network '{name}'")),
                _ => Err(format!("ambiguous neural network name '{name}'")),
            }
        }
    }
}

fn next_neural_network_id(networks: &[ProjectNeuralNetwork]) -> u64 {
    networks
        .iter()
        .map(|network| network.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1)
}

fn parse_nonnegative_usize(value: &EValue, label: &str) -> Result<usize, String> {
    match value {
        EValue::Number(value)
            if value.is_finite()
                && *value >= 0.0
                && value.fract() == 0.0
                && *value <= usize::MAX as f64 =>
        {
            Ok(*value as usize)
        }
        _ => Err(format!("{label} must be a non-negative integer")),
    }
}

fn parse_positive_neuron_count(value: &EValue) -> Result<usize, String> {
    let count = parse_nonnegative_usize(value, "neuron count")?;
    if count == 0 || count > NUM_NEURONS {
        return Err(format!("neuron count must be 1..={NUM_NEURONS}"));
    }
    Ok(count)
}

fn parse_u32_value(value: &EValue, label: &str) -> Result<u32, String> {
    match value {
        EValue::Number(value)
            if value.is_finite()
                && *value >= 0.0
                && value.fract() == 0.0
                && *value <= u32::MAX as f64 =>
        {
            Ok(*value as u32)
        }
        _ => Err(format!("{label} must be a non-negative integer")),
    }
}

fn parse_f32_value(value: &EValue, label: &str) -> Result<f32, String> {
    match value {
        EValue::Number(value)
            if value.is_finite() && *value >= f32::MIN as f64 && *value <= f32::MAX as f64 =>
        {
            Ok(*value as f32)
        }
        _ => Err(format!("{label} must be finite numeric")),
    }
}

fn parse_bool_value(value: &EValue, label: &str) -> Result<bool, String> {
    match value {
        EValue::Bool(value) => Ok(*value),
        EValue::Nil => Ok(false),
        EValue::Number(value) if value.is_finite() => Ok(*value != 0.0),
        _ => Err(format!("{label} expects a boolean")),
    }
}

fn parse_timebase_value(value: &EValue) -> Result<Timebase, String> {
    parse_timebase_arg(std::slice::from_ref(value), 0)
}

fn parse_neural_max_poly_selection(value: &EValue) -> Result<NeuralMaxPolySelection, String> {
    let name = match value {
        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
            value.trim().trim_start_matches(':').to_ascii_lowercase()
        }
        _ => {
            return Err(
                "max-poly selection expects deterministic, propagation, or random".to_string(),
            );
        }
    };
    match name.as_str() {
        "deterministic" | "ordered" | "first" => Ok(NeuralMaxPolySelection::Deterministic),
        "propagation" | "propagate" | "productive" | "impact" => {
            Ok(NeuralMaxPolySelection::Propagation)
        }
        "random" | "rand" => Ok(NeuralMaxPolySelection::Random),
        "loudest" | "loud" | "velocity" => Ok(NeuralMaxPolySelection::Loudest),
        "lowest-transpose" | "lowest" | "low-transpose" => {
            Ok(NeuralMaxPolySelection::LowestTranspose)
        }
        "highest-transpose" | "highest" | "high-transpose" => {
            Ok(NeuralMaxPolySelection::HighestTranspose)
        }
        "seed-first" | "seed" => Ok(NeuralMaxPolySelection::SeedFirst),
        _ => Err(
            "max-poly selection expects deterministic, propagation, random, loudest, \
             lowest-transpose, highest-transpose, or seed-first"
                .to_string(),
        ),
    }
}

fn neural_attr_name(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(name) => Some(name.to_ascii_lowercase()),
        EValue::Symbol(name) | EValue::String(name)
            if name.starts_with(':') || name.starts_with('@') =>
        {
            Some(
                name.trim_start_matches(':')
                    .trim_start_matches('@')
                    .to_ascii_lowercase(),
            )
        }
        _ => None,
    }
}

fn parse_neural_create_args(args: &[EValue]) -> Result<NeuralCreateOptions, String> {
    let mut name: Option<String> = None;
    let mut num_neurons: Option<usize> = None;
    let mut enabled = true;
    let mut weights_value: Option<EValue> = None;
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-create expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-create :{attr} expects a value"))?;
        match attr.as_str() {
            "name" => {
                name = match value {
                    EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                        Some(value.trim().to_string())
                    }
                    _ => return Err("neural-create :name expects string/symbol".to_string()),
                };
            }
            "neurons" | "num-neurons" => num_neurons = Some(parse_positive_neuron_count(value)?),
            "enabled" => enabled = parse_bool_value(value, "neural-create :enabled")?,
            "weights" => weights_value = Some(value.clone()),
            other => return Err(format!("neural-create unknown argument :{other}")),
        }
        idx += 1;
    }
    let name = name.ok_or_else(|| "neural-create requires :name".to_string())?;
    if name.is_empty() {
        return Err("neural-create :name cannot be empty".to_string());
    }
    let num_neurons = num_neurons.ok_or_else(|| "neural-create requires :neurons".to_string())?;
    let weights = weights_value
        .as_ref()
        .map(|value| parse_neural_weight_matrix(value, num_neurons))
        .transpose()?;
    Ok(NeuralCreateOptions {
        name,
        num_neurons,
        enabled,
        weights,
    })
}

fn parse_neural_set_args(args: &[EValue]) -> Result<NeuralSetEdits, String> {
    let mut edits = NeuralSetEdits::default();
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-set expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-set :{attr} expects a value"))?;
        match attr.as_str() {
            "name" => {
                edits.name = match value {
                    EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                        Some(value.trim().to_string())
                    }
                    _ => return Err("neural-set :name expects string/symbol".to_string()),
                };
            }
            "reset-bars" | "reset-interval-bars" => {
                edits.reset_interval_bars = Some(parse_f32_value(value, "reset bars")?)
            }
            "energy-decay" => edits.energy_decay = Some(parse_f32_value(value, "energy decay")?),
            "max-poly" => edits.max_poly = Some(parse_u32_value(value, "max-poly")?),
            "max-poly-selection" | "max-poly-mode" | "poly-selection" | "poly-mode" => {
                edits.max_poly_selection = Some(parse_neural_max_poly_selection(value)?)
            }
            other => return Err(format!("neural-set unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edits)
}

fn parse_neural_neuron_args(args: &[EValue]) -> Result<NeuralNeuronEdits, String> {
    let mut edits = NeuralNeuronEdits::default();
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-neuron expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-neuron :{attr} expects a value"))?;
        match attr.as_str() {
            "route" => {
                edits.route = Some(match value {
                    EValue::Nil | EValue::Bool(false) => None,
                    _ => Some(parse_nonnegative_usize(value, "route")?),
                });
            }
            "resolution" | "clock" => edits.resolution = Some(parse_timebase_value(value)?),
            "threshold" => edits.threshold = Some(parse_f32_value(value, "threshold")?.max(0.0)),
            "delay" | "delay-steps" => edits.delay_steps = Some(parse_u32_value(value, "delay")?),
            "quantize" => {
                edits.quantize = Some(match value {
                    EValue::Nil | EValue::Bool(false) => None,
                    _ => Some(parse_timebase_value(value)?),
                });
            }
            "transpose" => edits.transpose = Some(parse_f32_value(value, "transpose")?),
            "dampening" | "dampening-amount" => {
                edits.dampening_amount = Some(parse_f32_value(value, "dampening")?)
            }
            "dampening-recovery" | "recovery" => {
                edits.dampening_recovery = Some(parse_f32_value(value, "dampening recovery")?)
            }
            other => return Err(format!("neural-neuron unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edits)
}

fn parse_neural_weight_args(args: &[EValue]) -> Result<NeuralWeightEdit, String> {
    let mut from = None;
    let mut to = None;
    let mut value = None;
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-weight expects keyword arguments".to_string())?;
        idx += 1;
        let arg = args
            .get(idx)
            .ok_or_else(|| format!("neural-weight :{attr} expects a value"))?;
        match attr.as_str() {
            "from" => from = Some(parse_nonnegative_usize(arg, "from")?),
            "to" => to = Some(parse_nonnegative_usize(arg, "to")?),
            "value" | "amount" => value = Some(parse_f32_value(arg, "weight")?),
            other => return Err(format!("neural-weight unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(NeuralWeightEdit {
        from: from.ok_or_else(|| "neural-weight requires :from".to_string())?,
        to: to.ok_or_else(|| "neural-weight requires :to".to_string())?,
        value: value.ok_or_else(|| "neural-weight requires :value".to_string())?,
    })
}

fn parse_neural_reset_step_args(args: &[EValue]) -> Result<NeuralResetStepEdit, String> {
    if args.len() == 3 && matches!(args[0], EValue::Number(_)) {
        return Ok(NeuralResetStepEdit {
            track: parse_nonnegative_usize(&args[0], "track")?,
            step: parse_step_arg(args, 1)?,
            enabled: parse_bool_value(&args[2], "neural-reset-step")?,
        });
    }

    let mut track = None;
    let mut step = None;
    let mut enabled = None;
    let mut idx = 0;
    while idx < args.len() {
        if let Some(attr) = neural_attr_name(&args[idx]) {
            idx += 1;
            let value = args
                .get(idx)
                .ok_or_else(|| format!("neural-reset-step :{attr} expects a value"))?;
            match attr.as_str() {
                "track" => track = Some(parse_nonnegative_usize(value, "track")?),
                "step" => step = Some(parse_step_arg(args, idx)?),
                "enabled" | "value" => {
                    enabled = Some(parse_bool_value(value, "neural-reset-step")?)
                }
                other => return Err(format!("neural-reset-step unknown argument :{other}")),
            }
            idx += 1;
        } else {
            enabled = Some(parse_bool_value(&args[idx], "neural-reset-step")?);
            idx += 1;
        }
    }
    Ok(NeuralResetStepEdit {
        track: track.ok_or_else(|| "neural-reset-step requires :track".to_string())?,
        step: step.ok_or_else(|| "neural-reset-step requires :step".to_string())?,
        enabled: enabled.ok_or_else(|| "neural-reset-step requires enabled bool".to_string())?,
    })
}

fn parse_neural_weight_matrix(
    value: &EValue,
    expected_size: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let EValue::List(rows) = value else {
        return Err("neural weight matrix must be a list of rows".to_string());
    };
    let mut matrix = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.borrow();
        let EValue::List(cells) = &*row else {
            return Err("neural weight matrix rows must be lists".to_string());
        };
        let mut parsed_row = Vec::with_capacity(cells.len());
        for cell in cells {
            let cell = cell.borrow();
            match &*cell {
                EValue::Number(value)
                    if value.is_finite()
                        && *value >= f32::MIN as f64
                        && *value <= f32::MAX as f64 =>
                {
                    parsed_row.push(*value as f32)
                }
                _ => return Err("neural weight matrix cells must be numbers".to_string()),
            }
        }
        matrix.push(parsed_row);
    }
    validate_neural_matrix_shape(&matrix, expected_size)?;
    Ok(matrix)
}

fn validate_neural_matrix_shape(matrix: &[Vec<f32>], expected_size: usize) -> Result<(), String> {
    if matrix.len() != expected_size {
        return Err(format!(
            "neural weight matrix must have {expected_size} rows"
        ));
    }
    if matrix.iter().any(|row| row.len() != expected_size) {
        return Err(format!(
            "neural weight matrix must be {expected_size}x{expected_size}"
        ));
    }
    Ok(())
}

fn normalize_project_neural_network_shape(
    network: &mut ProjectNeuralNetwork,
) -> Result<(), String> {
    if network.num_neurons == 0 || network.num_neurons > NUM_NEURONS {
        return Err(format!("neural network size must be 1..={NUM_NEURONS}"));
    }
    network
        .neurons
        .resize_with(network.num_neurons, ProjectNeuron::default);
    network.neurons.truncate(network.num_neurons);
    if network.weights.len() != network.num_neurons
        || network
            .weights
            .iter()
            .any(|row| row.len() != network.num_neurons)
    {
        let mut normalized = vec![vec![0.0; network.num_neurons]; network.num_neurons];
        for (row_idx, row) in network.weights.iter().enumerate().take(network.num_neurons) {
            for (col_idx, value) in row.iter().enumerate().take(network.num_neurons) {
                normalized[row_idx][col_idx] = *value;
            }
        }
        network.weights = normalized;
    }
    Ok(())
}

fn apply_neural_set_edits(
    network: &mut ProjectNeuralNetwork,
    edits: &NeuralSetEdits,
) -> Result<(), String> {
    if let Some(name) = &edits.name {
        if name.is_empty() {
            return Err("neural-set :name cannot be empty".to_string());
        }
        network.name = name.clone();
    }
    if let Some(reset_interval_bars) = edits.reset_interval_bars {
        network.reset_interval_bars = reset_interval_bars.max(0.25);
    }
    if let Some(energy_decay) = edits.energy_decay {
        network.energy_decay = energy_decay.clamp(0.0, 1.0);
    }
    if let Some(max_poly) = edits.max_poly {
        network.max_poly = max_poly.max(1);
    }
    if let Some(max_poly_selection) = edits.max_poly_selection {
        network.max_poly_selection = max_poly_selection;
    }
    Ok(())
}

fn apply_neural_neuron_edits(
    neuron: &mut ProjectNeuron,
    edits: &NeuralNeuronEdits,
    track_count: usize,
) -> Result<(), String> {
    if let Some(route) = edits.route {
        if let Some(track) = route {
            if track >= track_count {
                return Err("route track out of range".to_string());
            }
        }
        neuron.route = route;
    }
    if let Some(resolution) = edits.resolution {
        neuron.resolution = resolution as u8;
    }
    if let Some(threshold) = edits.threshold {
        neuron.threshold = threshold.max(0.0);
    }
    if let Some(delay_steps) = edits.delay_steps {
        neuron.delay_steps = delay_steps;
    }
    if let Some(quantize) = edits.quantize {
        neuron.quantize = quantize.map(|timebase| timebase as u8);
    }
    if let Some(transpose) = edits.transpose {
        neuron.transpose = transpose;
    }
    if let Some(dampening_amount) = edits.dampening_amount {
        neuron.dampening_amount = dampening_amount.clamp(0.0, 1.0);
    }
    if let Some(dampening_recovery) = edits.dampening_recovery {
        neuron.dampening_recovery = dampening_recovery.clamp(0.0, 1.0);
    }
    Ok(())
}

fn neural_instrument_param_id(
    state: &crate::sequencer::SequencerState,
    track: usize,
    param_idx: usize,
) -> Result<ParamNodeId, String> {
    if track >= state.active_track_count() {
        return Err("target track out of range".to_string());
    }
    let slot = state
        .pattern
        .instrument_slots
        .get(track)
        .ok_or_else(|| "target track instrument slot out of range".to_string())?;
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return Err("instrument param out of range".to_string());
    }
    slot.param_node_id(param_idx)
        .ok_or_else(|| "instrument param has no live node identity".to_string())
}

fn neural_effect_param_id(
    state: &crate::sequencer::SequencerState,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<ParamNodeId, String> {
    if track >= state.active_track_count() {
        return Err("target track out of range".to_string());
    }
    let slot = state
        .pattern
        .effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
        .ok_or_else(|| "effect slot out of range".to_string())?;
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return Err("effect param out of range".to_string());
    }
    slot.param_node_id(param_idx)
        .ok_or_else(|| "effect param has no live node identity".to_string())
}

fn neural_neuron_mut(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
) -> Result<&mut ProjectNeuron, String> {
    normalize_project_neural_network_shape(network)?;
    if neuron_idx >= network.num_neurons {
        return Err("neuron index out of range".to_string());
    }
    network
        .neurons
        .get_mut(neuron_idx)
        .ok_or_else(|| "neuron index out of range".to_string())
}

fn upsert_neural_instrument_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    param_index: usize,
    param_id: ParamNodeId,
    value: f32,
) -> Result<(), String> {
    let network_id = network.id;
    let network_name = network.name.clone();
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    if let Some(existing) = neuron
        .output_overrides
        .instrument
        .iter_mut()
        .find(|entry| entry.target_track == target_track && entry.param_index == param_index)
    {
        existing.param_id = param_id;
        existing.value = value;
    } else {
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track,
                param_id,
                param_index,
                value,
            });
    }
    eprintln!(
        "[neural-plock] instrument network={network_id} name={network_name:?} neuron={neuron_idx} target_track={target_track} param={param_index} logical_id={} node_param_idx={} value={value}",
        param_id.logical_id, param_id.node_param_idx,
    );
    Ok(())
}

fn upsert_neural_effect_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    slot_index: usize,
    param_index: usize,
    param_id: ParamNodeId,
    value: f32,
) -> Result<(), String> {
    let network_id = network.id;
    let network_name = network.name.clone();
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    if let Some(existing) = neuron.output_overrides.effects.iter_mut().find(|entry| {
        entry.target_track == target_track
            && entry.slot_index == slot_index
            && entry.param_index == param_index
    }) {
        existing.param_id = param_id;
        existing.value = value;
    } else {
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track,
                slot_index,
                param_id,
                param_index,
                value,
            });
    }
    eprintln!(
        "[neural-plock] effect network={network_id} name={network_name:?} neuron={neuron_idx} target_track={target_track} slot={slot_index} param={param_index} logical_id={} node_param_idx={} value={value}",
        param_id.logical_id, param_id.node_param_idx,
    );
    Ok(())
}

fn clear_neural_instrument_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    param_index: usize,
) -> Result<bool, String> {
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    let before = neuron.output_overrides.instrument.len();
    neuron
        .output_overrides
        .instrument
        .retain(|entry| !(entry.target_track == target_track && entry.param_index == param_index));
    Ok(neuron.output_overrides.instrument.len() != before)
}

fn clear_neural_effect_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    slot_index: usize,
    param_index: usize,
) -> Result<bool, String> {
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    let before = neuron.output_overrides.effects.len();
    neuron.output_overrides.effects.retain(|entry| {
        !(entry.target_track == target_track
            && entry.slot_index == slot_index
            && entry.param_index == param_index)
    });
    Ok(neuron.output_overrides.effects.len() != before)
}

pub fn clear_neural_instrument_plock_by_network_id(
    state: &crate::sequencer::SequencerState,
    network_id: u64,
    neuron_idx: usize,
    target_track: usize,
    param_idx: usize,
) -> Result<bool, String> {
    state.edit_current_neural_networks(|networks| {
        let Some(network) = networks.iter_mut().find(|network| network.id == network_id) else {
            return Err("selected neural network was not found in the current pattern".to_string());
        };
        clear_neural_instrument_plock(network, neuron_idx, target_track, param_idx)
    })
}

pub fn clear_neural_effect_plock_by_network_id(
    state: &crate::sequencer::SequencerState,
    network_id: u64,
    neuron_idx: usize,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<bool, String> {
    state.edit_current_neural_networks(|networks| {
        let Some(network) = networks.iter_mut().find(|network| network.id == network_id) else {
            return Err("selected neural network was not found in the current pattern".to_string());
        };
        clear_neural_effect_plock(network, neuron_idx, target_track, slot_idx, param_idx)
    })
}

pub fn set_selected_neural_instrument_plocks(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    param_idx: usize,
    value: f32,
) -> Result<bool, String> {
    let current_pattern = state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
    let selected = selection
        .iter()
        .copied()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(false);
    }
    let param_id = neural_instrument_param_id(state, target_track, param_idx)?;
    let applied = state.edit_current_neural_networks(|networks| {
        let mut applied = 0_usize;
        for selected in &selected {
            let Some(network) = networks
                .iter_mut()
                .find(|network| network.id == selected.network_id)
            else {
                continue;
            };
            upsert_neural_instrument_plock(
                network,
                selected.neuron_idx,
                target_track,
                param_idx,
                param_id,
                value,
            )?;
            applied += 1;
        }
        Ok(applied)
    })?;
    if applied == 0 {
        return Err("selected neural network was not found in the current pattern".to_string());
    }
    Ok(true)
}

pub fn set_selected_neural_effect_plocks(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> Result<bool, String> {
    let current_pattern = state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
    let selected = selection
        .iter()
        .copied()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(false);
    }
    let param_id = neural_effect_param_id(state, target_track, slot_idx, param_idx)?;
    let applied = state.edit_current_neural_networks(|networks| {
        let mut applied = 0_usize;
        for selected in &selected {
            let Some(network) = networks
                .iter_mut()
                .find(|network| network.id == selected.network_id)
            else {
                continue;
            };
            upsert_neural_effect_plock(
                network,
                selected.neuron_idx,
                target_track,
                slot_idx,
                param_idx,
                param_id,
                value,
            )?;
            applied += 1;
        }
        Ok(applied)
    })?;
    if applied == 0 {
        return Err("selected neural network was not found in the current pattern".to_string());
    }
    Ok(true)
}

pub fn selected_neural_instrument_plock_value(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    param_idx: usize,
) -> Option<f32> {
    let current_pattern = state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
    let param_id = neural_instrument_param_id(state, target_track, param_idx).ok()?;
    let networks = state.current_neural_networks();
    selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .find_map(|selected| {
            networks
                .iter()
                .find(|network| network.id == selected.network_id)
                .and_then(|network| network.neurons.get(selected.neuron_idx))
                .and_then(|neuron| {
                    neuron.output_overrides.instrument.iter().find_map(|entry| {
                        (entry.target_track == target_track
                            && entry.param_index == param_idx
                            && entry.param_id == param_id)
                            .then_some(entry.value)
                    })
                })
        })
}

pub fn selected_neural_effect_plock_value(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Option<f32> {
    let current_pattern = state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
    let param_id = neural_effect_param_id(state, target_track, slot_idx, param_idx).ok()?;
    let networks = state.current_neural_networks();
    selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .find_map(|selected| {
            networks
                .iter()
                .find(|network| network.id == selected.network_id)
                .and_then(|network| network.neurons.get(selected.neuron_idx))
                .and_then(|neuron| {
                    neuron.output_overrides.effects.iter().find_map(|entry| {
                        (entry.target_track == target_track
                            && entry.slot_index == slot_idx
                            && entry.param_index == param_idx
                            && entry.param_id == param_id)
                            .then_some(entry.value)
                    })
                })
        })
}

fn neural_network_to_value(network: &ProjectNeuralNetwork) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("id".to_string(), lisp_number(network.id as f64));
    map.insert("name".to_string(), lisp_string(network.name.clone()));
    map.insert("enabled".to_string(), lisp_bool(network.enabled));
    map.insert(
        "num-neurons".to_string(),
        lisp_number(network.num_neurons as f64),
    );
    map.insert(
        "reset-bars".to_string(),
        lisp_number(network.reset_interval_bars as f64),
    );
    map.insert(
        "energy-decay".to_string(),
        lisp_number(network.energy_decay as f64),
    );
    map.insert("max-poly".to_string(), lisp_number(network.max_poly as f64));
    map.insert(
        "max-poly-selection".to_string(),
        lisp_string(network.max_poly_selection.as_str().to_string()),
    );
    map.insert(
        "weights".to_string(),
        lisp_value(lisp_list(
            network
                .weights
                .iter()
                .map(|row| {
                    lisp_list(
                        row.iter()
                            .map(|value| EValue::Number(*value as f64))
                            .collect(),
                    )
                })
                .collect(),
        )),
    );
    map.insert(
        "neurons".to_string(),
        lisp_value(lisp_list(
            network
                .neurons
                .iter()
                .enumerate()
                .map(|(idx, neuron)| neural_neuron_to_value(idx, neuron))
                .collect(),
        )),
    );
    EValue::Map(map)
}

pub fn selected_neural_neurons_to_value(selection: &BTreeSet<SelectedNeuralNeuron>) -> EValue {
    lisp_list(
        selection
            .iter()
            .map(|selected| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "pattern".to_string(),
                    lisp_number(selected.pattern_idx as f64),
                );
                map.insert(
                    "network-id".to_string(),
                    lisp_number(selected.network_id as f64),
                );
                map.insert(
                    "neuron".to_string(),
                    lisp_number(selected.neuron_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

fn neural_instrument_overrides_to_value(overrides: &[ProjectParamOverride]) -> EValue {
    lisp_list(
        overrides
            .iter()
            .map(|override_param| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "target-track".to_string(),
                    lisp_number(override_param.target_track as f64),
                );
                map.insert(
                    "param".to_string(),
                    lisp_number(override_param.param_index as f64),
                );
                map.insert(
                    "value".to_string(),
                    lisp_number(override_param.value as f64),
                );
                map.insert(
                    "logical-id".to_string(),
                    lisp_number(override_param.param_id.logical_id as f64),
                );
                map.insert(
                    "node-param-idx".to_string(),
                    lisp_number(override_param.param_id.node_param_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

fn neural_effect_overrides_to_value(overrides: &[ProjectEffectParamOverride]) -> EValue {
    lisp_list(
        overrides
            .iter()
            .map(|override_param| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "target-track".to_string(),
                    lisp_number(override_param.target_track as f64),
                );
                map.insert(
                    "slot".to_string(),
                    lisp_number(override_param.slot_index as f64),
                );
                map.insert(
                    "param".to_string(),
                    lisp_number(override_param.param_index as f64),
                );
                map.insert(
                    "value".to_string(),
                    lisp_number(override_param.value as f64),
                );
                map.insert(
                    "logical-id".to_string(),
                    lisp_number(override_param.param_id.logical_id as f64),
                );
                map.insert(
                    "node-param-idx".to_string(),
                    lisp_number(override_param.param_id.node_param_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

fn neural_neuron_to_value(idx: usize, neuron: &ProjectNeuron) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("index".to_string(), lisp_number(idx as f64));
    map.insert(
        "route".to_string(),
        lisp_value(
            neuron
                .route
                .map(|track| EValue::Number(track as f64))
                .unwrap_or(EValue::Nil),
        ),
    );
    map.insert(
        "resolution".to_string(),
        lisp_value(EValue::Keyword(
            neuron.resolution_timebase().label().to_string(),
        )),
    );
    map.insert("delay".to_string(), lisp_number(neuron.delay_steps as f64));
    map.insert(
        "threshold".to_string(),
        lisp_number(neuron.threshold as f64),
    );
    map.insert(
        "transpose".to_string(),
        lisp_number(neuron.transpose as f64),
    );
    map.insert(
        "quantize".to_string(),
        lisp_value(
            neuron
                .quantize_timebase()
                .map(|timebase| EValue::Keyword(timebase.label().to_string()))
                .unwrap_or(EValue::Nil),
        ),
    );
    map.insert(
        "dampening".to_string(),
        lisp_number(neuron.dampening_amount as f64),
    );
    map.insert(
        "dampening-recovery".to_string(),
        lisp_number(neuron.dampening_recovery as f64),
    );
    map.insert(
        "instrument-plocks".to_string(),
        lisp_value(neural_instrument_overrides_to_value(
            &neuron.output_overrides.instrument,
        )),
    );
    map.insert(
        "effect-plocks".to_string(),
        lisp_value(neural_effect_overrides_to_value(
            &neuron.output_overrides.effects,
        )),
    );
    EValue::Map(map)
}

#[derive(Default)]
struct GraphNodeEdit {
    resolution: Option<u8>,
    delay_steps: Option<u32>,
    quantize: Option<crate::graph::ProjectGraphQuantizeOverride>,
    route: Option<crate::graph::ProjectGraphRouteOverride>,
    seed_from: Option<crate::graph::ProjectGraphSeedFrom>,
}

struct GraphEdgeEdit {
    group: String,
    from: usize,
    to: usize,
    param: String,
    value: f64,
}

struct GraphEdgeQuery {
    group: String,
    from: usize,
    to: usize,
    param: String,
}

fn graph_key_string(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(k) | EValue::Symbol(k) | EValue::String(k) => Some(
            k.trim_start_matches(':')
                .trim_start_matches('@')
                .to_string(),
        ),
        _ => None,
    }
}

fn resolved_graph_overrides_for_manifest(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> Option<crate::graph::ProjectGraphOverrides> {
    state
        .current_graph_overrides()
        .into_iter()
        .find(|overrides| {
            overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
        })
}

fn graph_runtime_config_for_current_pattern(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> crate::graph::GraphRuntimeConfig {
    let graph_overrides = resolved_graph_overrides_for_manifest(state, manifest);
    manifest.runtime_config_with_overrides(graph_overrides.as_ref())
}

/// Reactive namespace that mirrors resolved graph node/edge values so the UI can
/// bind widgets directly (`bind-graph`) instead of shadowing every knob in a
/// per-node `defstate`. Writes flow back via `reactive-set` + `graph-*` setters.
const GRAPH_REACTIVE_NS: &str = "GRAPH";

struct GraphConfigCacheEntry {
    manifest_id: u64,
    pattern: usize,
    snapshot_version: u64,
    published_version: u64,
    config: Rc<crate::graph::GraphRuntimeConfig>,
}

thread_local! {
    // Materializing the runtime config locks the pattern bank, clones the override
    // vec, and allocates a HashMap per node. A single panel render resolves dozens
    // of node/edge values at the same (pattern, version); memoize so the whole
    // render collapses to one materialization. Any edit bumps snapshot_version and
    // invalidates the entry, so reads can never observe a stale config.
    static GRAPH_CONFIG_CACHE: RefCell<Option<GraphConfigCacheEntry>> = const { RefCell::new(None) };
}

fn cached_graph_runtime_config(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> Rc<crate::graph::GraphRuntimeConfig> {
    let pattern = state.current_pattern_index();
    let snapshot_version = state.scheduler_snapshot_version();
    let published_version = state.published_sequencers_version();
    GRAPH_CONFIG_CACHE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(entry) = slot.as_ref() {
            if entry.manifest_id == manifest.id
                && entry.pattern == pattern
                && entry.snapshot_version == snapshot_version
                && entry.published_version == published_version
            {
                return Rc::clone(&entry.config);
            }
        }
        let config = Rc::new(graph_runtime_config_for_current_pattern(state, manifest));
        *slot = Some(GraphConfigCacheEntry {
            manifest_id: manifest.id,
            pattern,
            snapshot_version,
            published_version,
            config: Rc::clone(&config),
        });
        config
    })
}

fn graph_node_reactive_field(manifest_id: u64, instance: usize, field: &str) -> String {
    format!("{manifest_id}|n{instance}|{field}")
}

fn graph_edge_reactive_field(manifest_id: u64, from: usize, to: usize, param: &str) -> String {
    format!("{manifest_id}|e{from}_{to}|{param}")
}

/// Seed the GRAPH reactive slot with `value` (a plain float write that does NOT
/// dirty bound widgets — safe to call during render) and return a reactive handle
/// pointing at the same slot. Re-running the producing lisp on a pattern switch
/// re-seeds the slot; live edits keep it current via `reactive-set`.
fn graph_seeded_reactive_ref(field: String, value: f64) -> EValue {
    eseqlisp::reactive::write_float_slot(GRAPH_REACTIVE_NS, &field, value);
    let slot = eseqlisp::reactive::reactive_float_slot(GRAPH_REACTIVE_NS, &field);
    EValue::ReactiveRef {
        namespace: GRAPH_REACTIVE_NS.to_string(),
        field,
        index: None,
        kind: eseqlisp::vm::BindingKind::Float,
        slot,
    }
}

/// Resolve a node field to a single float for `bind-graph`. `delay` is an
/// intrinsic; everything else falls through to behavioral params. Enum intrinsics
/// (route/resolution/quantize) are not scalars — callers must pass an options list
/// and go through the index path instead.
fn graph_node_numeric_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<f64, String> {
    match field {
        "delay" | "delay-steps" => {
            let value = resolved_graph_node_value(state, manifest, instance, field)?;
            graph_number(&value).ok_or_else(|| format!("bind-graph field :{field} is not numeric"))
        }
        "resolution" | "res" | "quantize" | "q" | "route" | "seed-from" => Err(format!(
            "bind-graph field :{field} is an enum; pass an options list to bind its index"
        )),
        _ => {
            let value = resolved_graph_param_value(state, manifest, instance, field)?;
            graph_number(&value).ok_or_else(|| format!("bind-graph param :{field} is not numeric"))
        }
    }
}

/// Render an enum node field to the label a dropdown would display, so it can be
/// matched against the author's options list. Centralizes the route/timebase
/// formatting that the lisp demo used to spell out as nested `if` ladders.
fn graph_node_display_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<String, String> {
    let value = resolved_graph_node_value(state, manifest, instance, field)?;
    Ok(match field {
        "route" => match value {
            EValue::Number(track) => format!("Track {}", track as usize + 1),
            _ => "Off".to_string(),
        },
        _ => match value {
            EValue::String(label) => label,
            EValue::Nil => "off".to_string(),
            EValue::Number(number) => graph_format_number(number),
            other => eseqlisp::vm::format_lisp_value(&other),
        },
    })
}

fn graph_format_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

fn graph_option_index(options: &EValue, display: &str) -> f64 {
    if let EValue::List(items) = options {
        for (index, item) in items.iter().enumerate() {
            let item = item.borrow();
            let matches = match &*item {
                EValue::String(label) => label == display,
                other => eseqlisp::vm::format_lisp_value(other) == display,
            };
            if matches {
                return index as f64;
            }
        }
    }
    0.0
}

/// Beats per bar for the demo's 4/4 reset clock, matching `graph_bars_or_beats`'s
/// `(bars n) -> n * 4` parse.
const GRAPH_BEATS_PER_BAR: f64 = 4.0;

fn graph_config_reactive_field(manifest_id: u64, field: &str) -> String {
    format!("{manifest_id}|cfg|{field}")
}

/// Resolve a sequencer-level config field (override-or-manifest) to a UI scalar.
/// `:reset-bars` reports bars (engine stores beats); `:max-poly` reports the cap.
fn resolved_graph_config_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
) -> Result<f64, String> {
    let overrides = resolved_graph_overrides_for_manifest(state, manifest);
    match field {
        "reset-bars" | "reset-every-bars" => {
            let beats = overrides
                .as_ref()
                .and_then(|o| o.reset_every_beats)
                .unwrap_or(manifest.reset_every_beats);
            Ok(beats / GRAPH_BEATS_PER_BAR)
        }
        "max-poly" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.max_poly)
                .unwrap_or(manifest.max_poly);
            Ok(value as f64)
        }
        other => Err(format!("graph config unknown field :{other}")),
    }
}

fn set_graph_config_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
    value: f64,
) -> Result<(), String> {
    state.edit_current_graph_overrides(|graphs| {
        let graph = ensure_graph_overrides(graphs, manifest);
        match field {
            "reset-bars" | "reset-every-bars" => {
                graph.reset_every_beats = Some((value * GRAPH_BEATS_PER_BAR).max(0.0));
            }
            "max-poly" => {
                graph.max_poly = Some(value.max(0.0).round() as u32);
            }
            other => return Err(format!("graph config unknown field :{other}")),
        }
        Ok(())
    })
}

fn graph_timebase_value(timebase: crate::sequencer::Timebase) -> EValue {
    EValue::String(timebase.label().to_string())
}

fn graph_route_value(route: Option<usize>) -> EValue {
    route
        .map(|track| EValue::Number(track as f64))
        .unwrap_or(EValue::Nil)
}

fn graph_seed_from_value(mask: u128) -> EValue {
    lisp_list(
        (0..128)
            .filter(|track| mask & (1_u128 << track) != 0)
            .map(|track| EValue::Number(track as f64))
            .collect(),
    )
}

fn resolved_graph_node_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let node = config
        .nodes
        .get(instance)
        .ok_or_else(|| "graph-node-value node index out of range".to_string())?;
    match field {
        "resolution" | "res" => Ok(graph_timebase_value(node.resolution)),
        "delay" | "delay-steps" => Ok(EValue::Number(node.delay_steps as f64)),
        "quantize" | "q" => Ok(node
            .quantize
            .map(graph_timebase_value)
            .unwrap_or_else(|| EValue::String("off".to_string()))),
        "route" => Ok(graph_route_value(node.route)),
        "seed-from" => Ok(graph_seed_from_value(node.seed_track_mask)),
        other => Err(format!("graph-node-value unknown field :{other}")),
    }
}

fn resolved_graph_param_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    param: &str,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let params = config
        .node_params
        .get(instance)
        .ok_or_else(|| "graph-param-value node index out of range".to_string())?;
    params
        .get(param)
        .copied()
        .or_else(|| manifest.node.param_default(param))
        .map(EValue::Number)
        .ok_or_else(|| format!("graph-param-value unknown param :{param}"))
}

fn parse_graph_edge_query(
    manifest: &crate::graph::GraphManifest,
    args: &[EValue],
) -> Result<GraphEdgeQuery, String> {
    let edge_set = manifest
        .edge_sets
        .first()
        .ok_or_else(|| "graph-edge-value requires an edge set".to_string())?;
    let default_group = crate::graph::edge_set_group_id(edge_set);
    if args.len() == 3 {
        let from = parse_nonnegative_usize(&args[0], "from")?;
        let to = parse_nonnegative_usize(&args[1], "to")?;
        let param = graph_key_string(&args[2])
            .ok_or_else(|| "graph-edge-value expects a param name".to_string())?;
        if from >= manifest.shape.num_nodes() || to >= manifest.shape.num_nodes() {
            return Err("graph-edge-value from/to index out of range".to_string());
        }
        return Ok(GraphEdgeQuery {
            group: default_group,
            from,
            to,
            param,
        });
    }

    let mut group = default_group.clone();
    let mut from = None;
    let mut to = None;
    let mut param = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-edge-value expects keyword/value pairs".to_string())?;
        idx += 1;
        match key.as_str() {
            "from" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :from expects a value".to_string())?;
                from = Some(parse_nonnegative_usize(value, "from")?);
                idx += 1;
            }
            "to" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :to expects a value".to_string())?;
                to = Some(parse_nonnegative_usize(value, "to")?);
                idx += 1;
            }
            "group" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :group expects a value".to_string())?;
                group = graph_key_string(value)
                    .ok_or_else(|| "graph-edge-value :group expects a symbol/string".to_string())?;
                idx += 1;
            }
            other => {
                if param.is_some() {
                    return Err("graph-edge-value expects one param".to_string());
                }
                param = Some(other.to_string());
            }
        }
    }
    let from = from.ok_or_else(|| "graph-edge-value requires :from".to_string())?;
    let to = to.ok_or_else(|| "graph-edge-value requires :to".to_string())?;
    if group != default_group {
        return Err(format!("graph-edge-value edge group not found: {group}"));
    }
    if from >= manifest.shape.num_nodes() || to >= manifest.shape.num_nodes() {
        return Err("graph-edge-value from/to index out of range".to_string());
    }
    Ok(GraphEdgeQuery {
        group,
        from,
        to,
        param: param.ok_or_else(|| "graph-edge-value requires an edge param".to_string())?,
    })
}

fn resolved_graph_edge_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    query: GraphEdgeQuery,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let edge = config
        .edges
        .iter()
        .find(|edge| edge.from == query.from && edge.to == query.to)
        .ok_or_else(|| "graph-edge-value edge not found".to_string())?;
    match query.param.as_str() {
        "weight" => Ok(EValue::Number(edge.weight)),
        "dampening" => Ok(EValue::Number(edge.dampening)),
        "delay" | "delay-steps" => Ok(EValue::Number(edge.delay_steps as f64)),
        other => Err(format!(
            "graph-edge-value unknown edge param :{} for group {}",
            other, query.group
        )),
    }
}

fn resolve_graph_manifest(
    state: &crate::sequencer::SequencerState,
    reference: &EValue,
) -> Result<crate::graph::GraphManifest, String> {
    let published = state.published_sequencers();
    match reference {
        EValue::Number(id) if id.is_finite() && *id >= 0.0 => {
            let id = *id as u64;
            published
                .into_iter()
                .filter_map(|published| published.graph)
                .find(|manifest| manifest.id == id)
                .ok_or_else(|| "graph sequencer id not found".to_string())
        }
        EValue::String(name) | EValue::Symbol(name) | EValue::Keyword(name) => {
            let name = name.trim_start_matches('@').trim_start_matches(':');
            published
                .into_iter()
                .filter_map(|published| published.graph)
                .find(|manifest| manifest.name == name)
                .ok_or_else(|| "graph sequencer name not found".to_string())
        }
        _ => Err("graph reference must be id or name".to_string()),
    }
}

fn graph_overrides_for_manifest<'a>(
    overrides: &'a [crate::graph::ProjectGraphOverrides],
    manifest: &crate::graph::GraphManifest,
) -> Option<&'a crate::graph::ProjectGraphOverrides> {
    overrides.iter().find(|overrides| {
        overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
    })
}

fn ensure_graph_overrides<'a>(
    graphs: &'a mut Vec<crate::graph::ProjectGraphOverrides>,
    manifest: &crate::graph::GraphManifest,
) -> &'a mut crate::graph::ProjectGraphOverrides {
    if let Some(idx) = graphs.iter().position(|graph| {
        graph.sequencer_id == manifest.id || graph.sequencer_name == manifest.name
    }) {
        return &mut graphs[idx];
    }
    graphs.push(crate::graph::ProjectGraphOverrides {
        sequencer_id: manifest.id,
        sequencer_name: manifest.name.clone(),
        ..crate::graph::ProjectGraphOverrides::default()
    });
    graphs.last_mut().expect("just pushed graph overrides")
}

fn ensure_graph_node_intrinsic<'a>(
    graph: &'a mut crate::graph::ProjectGraphOverrides,
    group: &str,
    instance: usize,
) -> &'a mut crate::graph::ProjectGraphNodeIntrinsicOverride {
    if let Some(idx) = graph
        .node_intrinsics
        .iter()
        .position(|node| node.group == group && node.instance == instance)
    {
        return &mut graph.node_intrinsics[idx];
    }
    graph
        .node_intrinsics
        .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
            group: group.to_string(),
            instance,
            resolution: None,
            delay_steps: None,
            quantize: None,
            route: None,
            seed_from: None,
        });
    graph
        .node_intrinsics
        .last_mut()
        .expect("just pushed graph node override")
}

fn parse_graph_route_override(
    value: &EValue,
) -> Result<crate::graph::ProjectGraphRouteOverride, String> {
    match graph_keyword(value).as_deref() {
        Some("none") | Some("nil") | Some("off") => {
            Ok(crate::graph::ProjectGraphRouteOverride::None)
        }
        _ => parse_nonnegative_usize(value, "route")
            .map(crate::graph::ProjectGraphRouteOverride::Track),
    }
}

fn parse_graph_seed_from(value: &EValue) -> Result<crate::graph::ProjectGraphSeedFrom, String> {
    match graph_keyword(value).as_deref() {
        Some("route") => return Ok(crate::graph::ProjectGraphSeedFrom::Route),
        _ => {}
    }
    match value {
        EValue::Number(_) => Ok(crate::graph::ProjectGraphSeedFrom::Tracks(vec![
            parse_nonnegative_usize(value, "seed-from")?,
        ])),
        EValue::List(_) => Ok(crate::graph::ProjectGraphSeedFrom::Tracks(
            graph_list_items(value)
                .unwrap_or_default()
                .iter()
                .map(|value| parse_nonnegative_usize(value, "seed-from track"))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        _ => Err("seed-from expects :route, track, or track list".to_string()),
    }
}

fn parse_graph_quantize_override(
    value: &EValue,
) -> Result<crate::graph::ProjectGraphQuantizeOverride, String> {
    match graph_keyword(value).as_deref() {
        Some("off") | Some("none") | Some("nil") | Some("false") => {
            Ok(crate::graph::ProjectGraphQuantizeOverride::Off)
        }
        _ => Ok(crate::graph::ProjectGraphQuantizeOverride::Timebase(
            graph_timebase(value)? as u8,
        )),
    }
}

fn parse_graph_node_edit(args: &[EValue]) -> Result<GraphNodeEdit, String> {
    let mut edit = GraphNodeEdit::default();
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-node expects keyword/value pairs".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("graph-node :{key} expects a value"))?;
        match key.as_str() {
            "resolution" | "res" => edit.resolution = Some(graph_timebase(value)? as u8),
            "delay" | "delay-steps" => edit.delay_steps = Some(parse_u32_value(value, "delay")?),
            "quantize" | "q" => edit.quantize = Some(parse_graph_quantize_override(value)?),
            "route" => edit.route = Some(parse_graph_route_override(value)?),
            "seed-from" => edit.seed_from = Some(parse_graph_seed_from(value)?),
            other => return Err(format!("graph-node unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edit)
}

fn apply_graph_node_edit(
    node: &mut crate::graph::ProjectGraphNodeIntrinsicOverride,
    edit: GraphNodeEdit,
) {
    if edit.resolution.is_some() {
        node.resolution = edit.resolution;
    }
    if edit.delay_steps.is_some() {
        node.delay_steps = edit.delay_steps;
    }
    if edit.quantize.is_some() {
        node.quantize = edit.quantize;
    }
    if edit.route.is_some() {
        node.route = edit.route;
    }
    if edit.seed_from.is_some() {
        node.seed_from = edit.seed_from;
    }
}

fn upsert_graph_node_param(
    graph: &mut crate::graph::ProjectGraphOverrides,
    group: &str,
    instance: usize,
    param: &str,
    value: f64,
) {
    if let Some(existing) = graph
        .node_params
        .iter_mut()
        .find(|entry| entry.group == group && entry.instance == instance && entry.param == param)
    {
        existing.value = value;
        return;
    }
    graph
        .node_params
        .push(crate::graph::ProjectGraphNodeParamOverride {
            group: group.to_string(),
            instance,
            param: param.to_string(),
            value,
        });
}

fn parse_graph_edge_edit(
    manifest: &crate::graph::GraphManifest,
    args: &[EValue],
) -> Result<GraphEdgeEdit, String> {
    let edge_set = manifest
        .edge_sets
        .first()
        .ok_or_else(|| "graph-edge requires an edge set".to_string())?;
    let mut group = crate::graph::edge_set_group_id(edge_set);
    let mut from = None;
    let mut to = None;
    let mut param = None;
    let mut value = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-edge expects keyword/value pairs".to_string())?;
        idx += 1;
        let arg = args
            .get(idx)
            .ok_or_else(|| format!("graph-edge :{key} expects a value"))?;
        match key.as_str() {
            "from" => from = Some(parse_nonnegative_usize(arg, "from")?),
            "to" => to = Some(parse_nonnegative_usize(arg, "to")?),
            "group" => {
                group = graph_key_string(arg)
                    .ok_or_else(|| "graph-edge :group expects a symbol/string".to_string())?
            }
            other => {
                param = Some(other.to_string());
                value = Some(graph_number(arg).ok_or("graph-edge param value must be numeric")?);
            }
        }
        idx += 1;
    }
    let from = from.ok_or_else(|| "graph-edge requires :from".to_string())?;
    let to = to.ok_or_else(|| "graph-edge requires :to".to_string())?;
    if from >= manifest.shape.num_nodes() || to >= manifest.shape.num_nodes() {
        return Err("graph-edge from/to index out of range".to_string());
    }
    Ok(GraphEdgeEdit {
        group,
        from,
        to,
        param: param.ok_or_else(|| "graph-edge requires an edge param".to_string())?,
        value: value.ok_or_else(|| "graph-edge requires an edge param value".to_string())?,
    })
}

fn upsert_graph_edge_param(graph: &mut crate::graph::ProjectGraphOverrides, edit: GraphEdgeEdit) {
    if let Some(existing) = graph.edge_params.iter_mut().find(|entry| {
        entry.group == edit.group
            && entry.from == edit.from
            && entry.to == edit.to
            && entry.param == edit.param
    }) {
        existing.value = edit.value;
        return;
    }
    graph
        .edge_params
        .push(crate::graph::ProjectGraphEdgeParamOverride {
            group: edit.group,
            from: edit.from,
            to: edit.to,
            param: edit.param,
            value: edit.value,
        });
}

fn graph_manifest_to_value(
    manifest: &crate::graph::GraphManifest,
    overrides: Option<&crate::graph::ProjectGraphOverrides>,
) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("id".to_string(), lisp_number(manifest.id as f64));
    map.insert("name".to_string(), lisp_string(manifest.name.clone()));
    map.insert(
        "nodes".to_string(),
        lisp_number(manifest.shape.num_nodes() as f64),
    );
    map.insert(
        "max-poly".to_string(),
        lisp_number(manifest.max_poly as f64),
    );
    map.insert(
        "max-poly-selection".to_string(),
        lisp_string(manifest.max_poly_selection.as_str().to_string()),
    );
    map.insert(
        "node-group".to_string(),
        lisp_string(manifest.node.name.clone()),
    );
    map.insert(
        "overrides".to_string(),
        lisp_number(
            overrides
                .map(|o| o.node_intrinsics.len() + o.node_params.len() + o.edge_params.len())
                .unwrap_or(0) as f64,
        ),
    );
    EValue::Map(map)
}

fn lisp_string(value: impl Into<String>) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::String(value.into())))
}

fn lisp_number(value: f64) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::Number(value)))
}

fn lisp_bool(value: bool) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::Bool(value)))
}

fn lisp_value(value: EValue) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(value))
}

fn lisp_list(items: Vec<EValue>) -> EValue {
    EValue::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

fn step_snapshot_to_value(step: usize, snapshot: StepSnapshot) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("step".to_string(), lisp_number(step as f64));
    map.insert("active".to_string(), lisp_bool(snapshot.active));
    map.insert(
        "duration".to_string(),
        lisp_number(snapshot.params[StepParam::Duration.index()] as f64),
    );
    map.insert(
        "velocity".to_string(),
        lisp_number(snapshot.params[StepParam::Velocity.index()] as f64),
    );
    map.insert(
        "speed".to_string(),
        lisp_number(snapshot.params[StepParam::Speed.index()] as f64),
    );
    map.insert(
        "transpose".to_string(),
        lisp_number(snapshot.params[StepParam::Transpose.index()] as f64),
    );
    map.insert(
        "pan".to_string(),
        lisp_number(snapshot.params[StepParam::Pan.index()] as f64),
    );
    map.insert(
        "delay".to_string(),
        lisp_number(snapshot.params[StepParam::Delay.index()] as f64),
    );
    map.insert(
        "chord".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord
                .into_iter()
                .map(|note| EValue::Number(note as f64))
                .collect(),
        )),
    );
    map.insert(
        "chord-durations".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord_durations
                .into_iter()
                .map(|duration| EValue::Number(duration as f64))
                .collect(),
        )),
    );
    map.insert(
        "chord-delays".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord_delays
                .into_iter()
                .map(|delay| EValue::Number(delay as f64))
                .collect(),
        )),
    );
    EValue::Map(map)
}

fn scratch_buffer_template() -> String {
    r#"; Scratch buffer for live sequencer scripting.
; C-x C-e eval s-expression at cursor
; C-x C-b eval whole buffer
; C-q quit scratch
; Examples:
;   (seq-track-steps)
;   (for-each |n| (seq-toggle-step n) (list 1 5 9 13))
;   (every :bar 2 '(seq-toggle-step 0))
;   (clear-hooks)

(seq-track-steps)
"#
    .to_string()
}

fn control_prelude_source() -> &'static str {
    r#"
(def empty? (xs) (= (len xs) 0))
(def map (fn xs)
  (if (empty? xs)
    '()
    (cons (fn (first xs))
          (map fn (rest xs)))))
(def filter (fn xs)
  (if (empty? xs)
    '()
    (if (fn (first xs))
      (cons (first xs) (filter fn (rest xs)))
      (filter fn (rest xs)))))
(def reduce (fn acc xs)
  (if (empty? xs)
    acc
    (reduce fn (fn acc (first xs)) (rest xs))))
(def for-each (fn xs)
  (if (empty? xs)
    nil
    (do
      (fn (first xs))
      (for-each fn (rest xs)))))
"#
}

fn new_eval_context(track: usize, cursor_step: usize) -> SharedSequencerEvalContext {
    Arc::new(Mutex::new(SequencerEvalContext { track, cursor_step }))
}

fn run_embedded_editor_session<F>(
    kind: CompileKind,
    path: PathBuf,
    sample_rate: u32,
    state: Arc<crate::sequencer::SequencerState>,
    track: Option<usize>,
    cursor_step: Option<usize>,
    mut apply_compiled: F,
) -> Option<(CompileResult, String, String)>
where
    F: FnMut(CompileKind, CompileResult, &str, &str) -> Result<(), String>,
{
    let init_src = read_eseqlisp_init_source();
    let mut runtime = Runtime::new();
    let track_count = state.active_track_count();
    register_sequencer_natives(
        &mut runtime,
        state,
        new_eval_context(track.unwrap_or(0), cursor_step.unwrap_or(0)),
        shared_native_metadata(
            fallback_effect_descriptors(track_count),
            fallback_instrument_descriptors(track_count),
        ),
    );
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            init_source_path: None,
            vim_mode: true,
        },
    );
    let initial = match std::fs::read_to_string(&path) {
        Ok(src) if !src.trim().is_empty() => None,
        _ => Some(default_template_for_kind(&kind).to_string()),
    };
    if editor
        .open_or_create_file_buffer_with_mode(&path, BufferMode::DGenLisp)
        .map_err(|e| eprintln!("Failed to open editor buffer '{}': {e:?}", path.display()))
        .is_err()
    {
        return None;
    }
    if let Some(initial) = initial {
        editor.active_buffer_mut().set_text(&initial);
    }

    let mut terminal = ratatui::init();
    let _restore_guard = RestoreTerminalGuard;
    let mut pending_job: Option<PendingCompileJob> = None;
    let mut quit_after_compile = false;
    let mut last_live_applied: Option<LiveAppliedCompile> = None;

    loop {
        if crossterm::event::poll(Duration::from_millis(16)).ok()? {
            match crossterm::event::read().ok()? {
                crossterm::event::Event::Key(key)
                    if !matches!(key.kind, crossterm::event::KeyEventKind::Release) =>
                {
                    editor.handle_key(key)
                }
                crossterm::event::Event::Resize(_, _) => editor.mark_needs_redraw(),
                _ => {}
            }
        }

        for command in editor.drain_host_commands() {
            match command {
                HostCommand::CompileInstrument {
                    source,
                    suggested_name,
                    path,
                    ..
                } if matches!(kind, CompileKind::Instrument) => {
                    let name = suggested_name
                        .or_else(|| {
                            path.as_ref()
                                .and_then(|p| source_name_from_path(&CompileKind::Instrument, p))
                        })
                        .unwrap_or_else(|| "untitled".to_string());
                    let save_path =
                        path.unwrap_or_else(|| editor_file_path(kind.clone(), Some(&name)));
                    std::fs::create_dir_all(save_path.parent().unwrap_or(Path::new("."))).ok();
                    if let Err(error) = std::fs::write(&save_path, &source) {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "failed to save '{}': {error}",
                            save_path.display()
                        )));
                        continue;
                    }
                    editor.handle_host_event(HostEvent::CommandStarted {
                        label: format!("compile instrument '{name}'"),
                    });
                    let (tx, rx) = std::sync::mpsc::channel();
                    let compile_source = source.clone();
                    let asset_base = save_path.parent().map(|p| p.to_path_buf());
                    std::thread::spawn(move || {
                        let result = compile_and_load_instrument_with_asset_base(
                            &compile_source,
                            sample_rate,
                            asset_base.as_deref(),
                        );
                        let _ = tx.send(result);
                    });
                    pending_job = Some(PendingCompileJob {
                        receiver: rx,
                        kind: CompileKind::Instrument,
                        name,
                        source,
                    });
                }
                HostCommand::CompileEffect {
                    source,
                    suggested_name,
                    path,
                    ..
                } if matches!(kind, CompileKind::Effect) => {
                    let name = suggested_name
                        .or_else(|| {
                            path.as_ref()
                                .and_then(|p| source_name_from_path(&CompileKind::Effect, p))
                        })
                        .unwrap_or_else(|| "untitled".to_string());
                    let save_path =
                        path.unwrap_or_else(|| editor_file_path(kind.clone(), Some(&name)));
                    std::fs::create_dir_all(save_path.parent().unwrap_or(Path::new("."))).ok();
                    if let Err(error) = std::fs::write(&save_path, &source) {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "failed to save '{}': {error}",
                            save_path.display()
                        )));
                        continue;
                    }
                    editor.handle_host_event(HostEvent::CommandStarted {
                        label: format!("compile effect '{name}'"),
                    });
                    let (tx, rx) = std::sync::mpsc::channel();
                    let compile_source = source.clone();
                    std::thread::spawn(move || {
                        let result = compile_and_load(&compile_source, sample_rate);
                        let _ = tx.send(result);
                    });
                    pending_job = Some(PendingCompileJob {
                        receiver: rx,
                        kind: CompileKind::Effect,
                        name,
                        source,
                    });
                }
                HostCommand::Custom { name, payload } => {
                    if name == "sync-current-buffer" {
                        continue;
                    } else if name == "compile-current" {
                        let source = editor.active_buffer().text();
                        let save_path = editor
                            .active_buffer()
                            .path
                            .clone()
                            .unwrap_or_else(|| editor_file_path(kind.clone(), None));
                        let suggested_name = source_name_from_path(&kind, &save_path);
                        let command = match kind {
                            CompileKind::Instrument => HostCommand::CompileInstrument {
                                source,
                                suggested_name,
                                buffer_id: editor.active_buffer().id,
                                path: Some(save_path),
                            },
                            CompileKind::Effect => HostCommand::CompileEffect {
                                source,
                                suggested_name,
                                buffer_id: editor.active_buffer().id,
                                path: Some(save_path),
                            },
                        };

                        match command {
                            HostCommand::CompileInstrument {
                                source,
                                suggested_name,
                                path,
                                ..
                            } => {
                                let name = suggested_name
                                    .or_else(|| {
                                        path.as_ref().and_then(|p| {
                                            source_name_from_path(&CompileKind::Instrument, p)
                                        })
                                    })
                                    .unwrap_or_else(|| "untitled".to_string());
                                let save_path = path.unwrap_or_else(|| {
                                    editor_file_path(CompileKind::Instrument, Some(&name))
                                });
                                std::fs::create_dir_all(
                                    save_path.parent().unwrap_or(Path::new(".")),
                                )
                                .ok();
                                if let Err(error) = std::fs::write(&save_path, &source) {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "failed to save '{}': {error}",
                                        save_path.display()
                                    )));
                                    continue;
                                }
                                editor.handle_host_event(HostEvent::CommandStarted {
                                    label: format!("compile instrument '{name}'"),
                                });
                                let (tx, rx) = std::sync::mpsc::channel();
                                let compile_source = source.clone();
                                let asset_base = save_path.parent().map(|p| p.to_path_buf());
                                std::thread::spawn(move || {
                                    let result = compile_and_load_instrument_with_asset_base(
                                        &compile_source,
                                        sample_rate,
                                        asset_base.as_deref(),
                                    );
                                    let _ = tx.send(result);
                                });
                                pending_job = Some(PendingCompileJob {
                                    receiver: rx,
                                    kind: CompileKind::Instrument,
                                    name,
                                    source,
                                });
                            }
                            HostCommand::CompileEffect {
                                source,
                                suggested_name,
                                path,
                                ..
                            } => {
                                let name = suggested_name
                                    .or_else(|| {
                                        path.as_ref().and_then(|p| {
                                            source_name_from_path(&CompileKind::Effect, p)
                                        })
                                    })
                                    .unwrap_or_else(|| "untitled".to_string());
                                let save_path = path.unwrap_or_else(|| {
                                    editor_file_path(CompileKind::Effect, Some(&name))
                                });
                                std::fs::create_dir_all(
                                    save_path.parent().unwrap_or(Path::new(".")),
                                )
                                .ok();
                                if let Err(error) = std::fs::write(&save_path, &source) {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "failed to save '{}': {error}",
                                        save_path.display()
                                    )));
                                    continue;
                                }
                                editor.handle_host_event(HostEvent::CommandStarted {
                                    label: format!("compile effect '{name}'"),
                                });
                                let (tx, rx) = std::sync::mpsc::channel();
                                let compile_source = source.clone();
                                std::thread::spawn(move || {
                                    let result = compile_and_load(&compile_source, sample_rate);
                                    let _ = tx.send(result);
                                });
                                pending_job = Some(PendingCompileJob {
                                    receiver: rx,
                                    kind: CompileKind::Effect,
                                    name,
                                    source,
                                });
                            }
                            HostCommand::Custom { .. } => {}
                        }
                    } else {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "host command '{name}' ignored: {payload:?}"
                        )));
                    }
                }
                _ => {}
            }
        }

        if let Some(job) = pending_job.take() {
            match job.receiver.try_recv() {
                Ok(Ok(result)) => {
                    let compiled_name = job.name.clone();
                    let kind = job.kind.clone();
                    editor.handle_host_event(HostEvent::CompileFinished {
                        kind: kind.clone(),
                        success: true,
                        name: Some(compiled_name),
                        diagnostics: None,
                    });
                    if quit_after_compile {
                        return Some((result, job.name, job.source));
                    } else if let Err(error) = apply_compiled(kind, result, &job.name, &job.source)
                    {
                        editor.handle_host_event(HostEvent::Error(error));
                    } else {
                        last_live_applied = Some(LiveAppliedCompile {
                            kind: job.kind,
                            name: job.name,
                            source: job.source,
                        });
                    }
                }
                Ok(Err(error)) => {
                    editor.handle_host_event(HostEvent::CompileFinished {
                        kind: job.kind,
                        success: false,
                        name: Some(job.name),
                        diagnostics: Some(error),
                    });
                    if quit_after_compile {
                        quit_after_compile = false;
                        editor.clear_quit_request();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    pending_job = Some(job);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    editor
                        .handle_host_event(HostEvent::Error("compile worker crashed".to_string()));
                }
            }
        }

        if editor.needs_redraw() {
            terminal
                .draw(|f| {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    let viewport_width = cols as usize;
                    let viewport_height = (rows as usize).saturating_sub(3);
                    let render_frame = eseq_frame::build_render_frame(
                        &mut editor,
                        viewport_width,
                        viewport_height,
                    );
                    eseq_tui::render(f, &render_frame);
                })
                .ok()?;
            editor.clear_needs_redraw();
        }

        if editor.should_quit() {
            if pending_job.is_some() {
                quit_after_compile = true;
            } else {
                let buffer = editor.active_buffer();
                let source = buffer.text();
                let save_path = buffer
                    .path
                    .clone()
                    .unwrap_or_else(|| editor_file_path(kind.clone(), Some(&buffer.name)));
                if let Err(error) =
                    std::fs::create_dir_all(save_path.parent().unwrap_or(Path::new(".")))
                {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "failed to create parent dir for '{}': {error}",
                        save_path.display()
                    )));
                    editor.clear_quit_request();
                    continue;
                }
                if let Err(error) = std::fs::write(&save_path, &source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "failed to save '{}': {error}",
                        save_path.display()
                    )));
                    editor.clear_quit_request();
                    continue;
                }

                let name = source_name_from_path(&kind, &save_path)
                    .unwrap_or_else(|| "untitled".to_string());

                if last_live_applied
                    .as_ref()
                    .map(|applied| {
                        applied.kind == kind && applied.name == name && applied.source == source
                    })
                    .unwrap_or(false)
                {
                    return None;
                }

                editor.handle_host_event(HostEvent::CommandStarted {
                    label: match kind {
                        CompileKind::Instrument => format!("compile instrument '{name}'"),
                        CompileKind::Effect => format!("compile effect '{name}'"),
                    },
                });

                let (tx, rx) = std::sync::mpsc::channel();
                match kind {
                    CompileKind::Instrument => {
                        let compile_source = source.clone();
                        let asset_base = editor_file_path(kind.clone(), Some(&name))
                            .parent()
                            .map(|p| p.to_path_buf());
                        std::thread::spawn(move || {
                            let result = compile_and_load_instrument_with_asset_base(
                                &compile_source,
                                sample_rate,
                                asset_base.as_deref(),
                            );
                            let _ = tx.send(result);
                        });
                    }
                    CompileKind::Effect => {
                        let compile_source = source.clone();
                        std::thread::spawn(move || {
                            let result = compile_and_load(&compile_source, sample_rate);
                            let _ = tx.send(result);
                        });
                    }
                }

                pending_job = Some(PendingCompileJob {
                    receiver: rx,
                    kind: kind.clone(),
                    name,
                    source,
                });
                quit_after_compile = true;
                editor.clear_quit_request();
            }
        }
    }
}

pub fn run_embedded_scratch_flow(
    track: usize,
    cursor_step: usize,
    initial_text: &str,
    initial_cursor: (usize, usize),
    mut control_runtime: ScratchControlRuntime,
    mut on_loop_event: impl FnMut(&mut Editor, Option<(&str, &EValue)>) -> Option<String>,
) -> Option<(String, (usize, usize), ScratchControlRuntime)> {
    control_runtime.set_position(track, cursor_step);
    let (
        runtime,
        context,
        metadata,
        accumulators,
        midi_fx,
        pending_midi_fx_params,
        midi_fx_state,
        accumulator_eval,
        sequencers,
        generator_tick,
        graph_node,
    ) = control_runtime.into_parts();
    let init_src = read_eseqlisp_init_source();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            init_source_path: None,
            vim_mode: true,
        },
    );
    let initial = if initial_text.trim().is_empty() {
        scratch_buffer_template()
    } else {
        initial_text.to_string()
    };
    editor.open_scratch_buffer_with_mode("*scratch*", &initial, BufferMode::ESeqLisp);
    {
        let buffer = editor.active_buffer_mut();
        buffer.path = Some(PathBuf::from(".eseqlisp-scratch"));
        let row = initial_cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = initial_cursor.1.min(buffer.lines[row].len());
        buffer.cursor = (row, col);
    }

    let mut terminal = ratatui::init();
    let _restore_guard = RestoreTerminalGuard;

    loop {
        if crossterm::event::poll(Duration::from_millis(16)).ok() == Some(true) {
            match crossterm::event::read().ok() {
                Some(crossterm::event::Event::Key(key))
                    if !matches!(key.kind, crossterm::event::KeyEventKind::Release) =>
                {
                    editor.handle_key(key)
                }
                Some(crossterm::event::Event::Resize(_, _)) => editor.mark_needs_redraw(),
                _ => {}
            }
        }

        for command in editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                if name == "sync-current-buffer" {
                    let _ = on_loop_event(&mut editor, Some((&name, &payload)));
                    continue;
                }
                if let Some(status) = on_loop_event(&mut editor, Some((&name, &payload))) {
                    editor.handle_host_event(HostEvent::Status(status));
                } else {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "host command '{name}' ignored: {payload:?}"
                    )));
                }
            }
        }

        let _ = on_loop_event(&mut editor, None);

        if editor.needs_redraw() {
            if terminal
                .draw(|f| {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    let viewport_width = cols as usize;
                    let viewport_height = (rows as usize).saturating_sub(3);
                    let render_frame = eseq_frame::build_render_frame(
                        &mut editor,
                        viewport_width,
                        viewport_height,
                    );
                    eseq_tui::render(f, &render_frame);
                })
                .is_err()
            {
                break;
            }
            editor.clear_needs_redraw();
        }

        if editor.should_quit() {
            let buffer = editor.active_buffer();
            return Some((
                buffer.text(),
                buffer.cursor,
                ScratchControlRuntime::from_parts(
                    editor.into_runtime(),
                    context,
                    metadata,
                    accumulators,
                    midi_fx,
                    pending_midi_fx_params,
                    midi_fx_state,
                    accumulator_eval,
                    sequencers,
                    generator_tick,
                    graph_node,
                ),
            ));
        }
    }
    None
}

pub fn scratch_runtime_with_fallbacks(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
) -> ScratchControlRuntime {
    let track_count = state.active_track_count().max(1);
    let (effect_descriptors, instrument_descriptors) = state.scratch_runtime_descriptors();
    let effect_descriptors = if effect_descriptors.is_empty() {
        fallback_effect_descriptors(track_count)
    } else {
        effect_descriptors
    };
    let instrument_descriptors = if instrument_descriptors.is_empty() {
        fallback_instrument_descriptors(track_count)
    } else {
        instrument_descriptors
    };
    let mut runtime = ScratchControlRuntime::new(
        state,
        effect_descriptors,
        instrument_descriptors,
        track,
        cursor_step,
    );
    runtime.set_theme_sync_enabled(false);
    runtime
}

pub fn load_midi_fx_library_source() -> String {
    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                let dsp = path.join("dsp.lisp");
                if dsp.exists() {
                    if let (Ok(rel), Ok(src)) =
                        (path.strip_prefix(root), std::fs::read_to_string(&dsp))
                    {
                        out.push((rel.to_string_lossy().replace('\\', "/"), src));
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let root = Path::new("midi-fx");
    let mut sources = Vec::new();
    collect(root, root, &mut sources);
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
        .into_iter()
        .map(|(name, src)| format!("; midi-fx/{name}/dsp.lisp\n{src}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn midi_fx_library_source_with_user_source(user_source: &str) -> String {
    let library = load_midi_fx_library_source();
    if library.trim().is_empty() {
        user_source.to_string()
    } else if user_source.trim().is_empty() {
        library
    } else {
        format!("{library}\n; *scratch*\n{user_source}")
    }
}

pub fn load_midi_fx_descriptors() -> Vec<EffectDescriptor> {
    let source = load_midi_fx_library_source();
    if source.trim().is_empty() {
        return Vec::new();
    }

    let cache = MIDI_FX_DESCRIPTOR_CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().expect("midi fx descriptor cache poisoned");
        if let Some(cached) = guard.as_ref().filter(|cached| cached.source == source) {
            return cached.descriptors.clone();
        }
    }

    let state = Arc::new(crate::sequencer::SequencerState::new(
        1,
        vec![crate::sequencer::default_empty_effect_chain()],
    ));
    let mut runtime = ScratchControlRuntime::new(
        Arc::clone(&state),
        fallback_effect_descriptors(1),
        fallback_instrument_descriptors(1),
        0,
        0,
    );
    let descriptors = if runtime.eval(&source).is_err() {
        Vec::new()
    } else {
        runtime.midi_fx_descriptors()
    };
    *cache.lock().expect("midi fx descriptor cache poisoned") = Some(MidiFxDescriptorCache {
        source,
        descriptors: descriptors.clone(),
    });
    descriptors
}

pub fn load_midi_fx_descriptor(name: &str) -> Option<EffectDescriptor> {
    load_midi_fx_descriptors()
        .into_iter()
        .find(|desc| desc.name.eq_ignore_ascii_case(name))
}

pub fn eval_sequencer_control(
    code: &str,
    state: Arc<crate::sequencer::SequencerState>,
    track: Option<usize>,
    cursor_step: Option<usize>,
) -> Result<Option<EValue>, String> {
    let mut runtime = Runtime::new();
    let track_count = state.active_track_count();
    register_sequencer_natives(
        &mut runtime,
        state,
        new_eval_context(track.unwrap_or(0), cursor_step.unwrap_or(0)),
        shared_native_metadata(
            fallback_effect_descriptors(track_count),
            fallback_instrument_descriptors(track_count),
        ),
    );
    runtime
        .eval_str(control_prelude_source())
        .map_err(|e| format!("{e:?}"))?;
    runtime.eval_str(code).map_err(|e| format!("{e:?}"))
}

pub fn run_embedded_effect_editor_flow<F>(
    sample_rate: u32,
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    existing_name: Option<&str>,
    apply_compiled: F,
) -> Option<EffectEditResult>
where
    F: FnMut(CompileKind, CompileResult, &str, &str) -> Result<(), String>,
{
    let path = editor_file_path(CompileKind::Effect, existing_name);
    let (result, name, source) = run_embedded_editor_session(
        CompileKind::Effect,
        path,
        sample_rate,
        state,
        Some(track),
        None,
        apply_compiled,
    )?;
    Some(EffectEditResult {
        manifest: result.manifest,
        lib: result.lib,
        source,
        name,
    })
}

pub fn run_embedded_instrument_editor_flow<F>(
    sample_rate: u32,
    state: Arc<crate::sequencer::SequencerState>,
    track: Option<usize>,
    existing_name: Option<&str>,
    apply_compiled: F,
) -> Option<InstrumentEditResult>
where
    F: FnMut(CompileKind, CompileResult, &str, &str) -> Result<(), String>,
{
    let path = editor_file_path(CompileKind::Instrument, existing_name);
    let (result, name, source) = run_embedded_editor_session(
        CompileKind::Instrument,
        path,
        sample_rate,
        state,
        track,
        None,
        apply_compiled,
    )?;
    let params = result.manifest.params.clone();
    Some(InstrumentEditResult {
        manifest: result.manifest,
        lib: result.lib,
        source,
        params,
        name,
    })
}

/// Run the instrument edit → compile → name → save flow.
/// Called while terminal is in normal (non-raw) mode.
/// Does NOT wire nodes — the caller handles graph wiring.
pub fn run_instrument_editor_flow(
    last_source: &str,
    existing_name: Option<&str>,
    sample_rate: u32,
) -> Option<InstrumentEditResult> {
    let initial = if last_source.is_empty() {
        INSTRUMENT_TEMPLATE.to_string()
    } else {
        last_source.to_string()
    };

    let mut source = initial;

    loop {
        match edit_text(&source) {
            Ok(edited) => {
                source = edited;
            }
            Err(e) => {
                eprintln!("Editor error: {e}");
                return None;
            }
        }

        print!("Compiling instrument...");
        io::stdout().flush().ok();

        match compile_instrument(&source, sample_rate) {
            Ok(json) => match parse_manifest(&json) {
                Ok(manifest) => match load_dylib(&manifest.dylib_path) {
                    Ok(lib) => {
                        println!(" OK!");
                        let n = manifest.params.len();
                        if n > 0 {
                            println!("  Parameters:");
                            for p in &manifest.params {
                                println!(
                                    "    {} = {} [{}, {}]{}",
                                    p.name,
                                    p.default,
                                    p.min,
                                    p.max,
                                    p.unit
                                        .as_deref()
                                        .map(|u| format!(" {u}"))
                                        .unwrap_or_default()
                                );
                            }
                        }

                        let default_name = existing_name.unwrap_or("");
                        if default_name.is_empty() {
                            print!("\nInstrument name: ");
                        } else {
                            print!("\nInstrument name [{}]: ", default_name);
                        }
                        io::stdout().flush().ok();
                        let mut name_buf = String::new();
                        std::io::stdin().read_line(&mut name_buf).ok();
                        let name_input = name_buf.trim();
                        let name = if name_input.is_empty() {
                            if default_name.is_empty() {
                                "untitled".to_string()
                            } else {
                                default_name.to_string()
                            }
                        } else {
                            sanitize_effect_name(name_input)
                        };

                        match save_instrument(&name, &source) {
                            Ok(()) => println!("Saved to instruments/{}.lisp", name),
                            Err(e) => eprintln!("Warning: failed to save: {e}"),
                        }

                        println!("\nInstrument '{}' compiled successfully.", name);
                        let params = manifest.params.clone();
                        return Some(InstrumentEditResult {
                            manifest,
                            lib,
                            source,
                            params,
                            name,
                        });
                    }
                    Err(e) => eprintln!(" Failed to load dylib: {e}"),
                },
                Err(e) => eprintln!(" Failed to parse manifest: {e}"),
            },
            Err(e) => {
                println!();
                eprintln!("Compile error:\n{e}");
            }
        }

        eprint!("\nPress Enter to re-edit, or 'q' + Enter to cancel: ");
        io::stdout().flush().ok();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
        if buf.trim() == "q" {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clear_neural_effect_plock_by_network_id, clear_neural_instrument_plock_by_network_id,
        compile_instrument, compile_instrument_with_asset_base, effect_has_host_modulation,
        effect_sidechain_inputs, fallback_effect_descriptors, fallback_instrument_descriptors,
        new_eval_context, parse_manifest, read_eseqlisp_init_source,
        register_graph_authoring_natives, register_sequencer_natives,
        scratch_runtime_with_fallbacks, selected_neural_instrument_plock_value,
        set_selected_neural_instrument_plocks, shared_native_metadata, AccumulatorNoteSpan,
        DGenParam, DGenSidechainInput, ScratchControlRuntime, SelectedNeuralNeuron,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
    use crate::neural::{NeuralMaxPolySelection, ParamNodeId};
    use crate::scheduled_event::{
        ScheduledEffectParam, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    };
    use crate::sequencer::{
        default_empty_effect_chain, PublishedSequencer, SequencerState, StepParam, Timebase,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::vm::Value;
    use eseqlisp::{BufferMode, Editor, EditorConfig, Runtime};
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── graph-mode manifest parsing ──
    fn gv_num(x: f64) -> Value {
        Value::Number(x)
    }
    fn gv_kw(s: &str) -> Value {
        Value::Keyword(s.to_string())
    }
    fn gv_sym(s: &str) -> Value {
        Value::Symbol(s.to_string())
    }
    fn gv_list(items: Vec<Value>) -> Value {
        Value::List(
            items
                .into_iter()
                .map(|v| Rc::new(RefCell::new(v)))
                .collect(),
        )
    }

    fn sample_graph_args() -> Vec<Value> {
        vec![
            gv_sym("neural"),
            gv_kw("shape"),
            gv_list(vec![gv_sym("line"), gv_num(2.0)]),
            gv_kw("energy-decay"),
            gv_num(0.9),
            gv_kw("max-poly"),
            gv_num(4.0),
            gv_kw("max-poly-selection"),
            gv_kw("propagation"),
            gv_kw("reset-every"),
            gv_list(vec![gv_sym("bars"), gv_num(4.0)]),
            gv_list(vec![
                gv_sym("def-node"),
                gv_sym("nrn"),
                gv_kw("resolution"),
                gv_kw("16"),
                gv_kw("delay"),
                gv_num(1.0),
                gv_kw("seed-from"),
                gv_num(0.0),
                gv_kw("quantize"),
                gv_kw("off"),
                gv_kw("reduce"),
                gv_sym("sum"),
                gv_kw("params"),
                gv_list(vec![
                    gv_list(vec![
                        gv_sym("threshold"),
                        gv_kw("float"),
                        gv_num(0.0),
                        gv_num(4.0),
                        gv_kw("default"),
                        gv_num(1.0),
                    ]),
                    gv_list(vec![
                        gv_sym("transpose"),
                        gv_kw("int"),
                        gv_num(-24.0),
                        gv_num(24.0),
                        gv_kw("default"),
                        gv_num(0.0),
                    ]),
                ]),
                gv_kw("state"),
                gv_list(vec![gv_list(vec![
                    gv_sym("energy"),
                    gv_kw("leak"),
                    gv_list(vec![gv_sym("per-step"), gv_kw("energy-decay")]),
                ])]),
                gv_kw("update"),
                gv_list(vec![
                    gv_sym(">="),
                    gv_list(vec![gv_sym("node-state"), gv_sym("self"), gv_kw("energy")]),
                    gv_num(1.0),
                ]),
            ]),
            gv_list(vec![
                gv_sym("edges"),
                gv_kw("from"),
                gv_sym("nrn"),
                gv_kw("to"),
                gv_sym("nrn"),
                gv_kw("topology"),
                gv_list(vec![gv_sym("all-to-all")]),
                gv_kw("params"),
                gv_list(vec![gv_list(vec![
                    gv_sym("weight"),
                    gv_kw("float"),
                    gv_num(-1.0),
                    gv_num(1.0),
                    gv_kw("default"),
                    gv_num(0.5),
                ])]),
            ]),
        ]
    }

    #[test]
    fn graph_mode_detected_by_def_node() {
        assert!(super::graph_mode_present(&sample_graph_args()));
        // A tick-style arg list (no def-node) is not graph mode.
        let tick_args = vec![
            Value::Symbol("liebezeit".into()),
            Value::Keyword("resolution".into()),
            Value::Keyword("16".into()),
            Value::Keyword("tick".into()),
            gv_list(vec![gv_sym("lambda"), gv_list(vec![])]),
        ];
        assert!(!super::graph_mode_present(&tick_args));
    }

    #[test]
    fn parse_graph_manifest_extracts_full_shape() {
        use crate::graph::{LeakSpec, Reduce, SeedFrom, ShapeSpec, Topology};
        use crate::sequencer::Timebase;

        let manifest = super::parse_graph_manifest(&sample_graph_args()).expect("parse");
        assert_eq!(manifest.name, "neural");
        assert_eq!(manifest.shape, ShapeSpec::Line(2));
        assert_eq!(manifest.energy_decay, 0.9);
        assert_eq!(manifest.max_poly, 4);
        assert_eq!(
            manifest.max_poly_selection,
            NeuralMaxPolySelection::Propagation
        );
        assert_eq!(manifest.reset_every_beats, 16.0); // (bars 4) @ 4/4

        let node = &manifest.node;
        assert_eq!(node.name, "nrn");
        assert_eq!(node.resolution, Timebase::Sixteenth);
        assert_eq!(node.delay_steps, 1);
        assert_eq!(node.quantize, None);
        assert_eq!(node.reduce, Reduce::Sum);
        assert_eq!(node.seed_from, SeedFrom::Tracks(vec![0]));
        assert_eq!(node.param_default("threshold"), Some(1.0));
        let transpose = node.params.iter().find(|p| p.name == "transpose").unwrap();
        assert!(transpose.is_int);
        assert_eq!(transpose.default, 0.0);
        assert_eq!(node.state.len(), 1);
        assert_eq!(node.state[0].name, "energy");
        assert_eq!(node.state[0].leak, Some(LeakSpec::PerStepEnergyDecay));
        assert!(node.update_source.as_deref().unwrap().contains(">="));

        assert_eq!(manifest.edge_sets.len(), 1);
        let edges = &manifest.edge_sets[0];
        assert_eq!(edges.from, "nrn");
        assert_eq!(edges.to, "nrn");
        assert_eq!(edges.topology, Topology::AllToAll);
        assert_eq!(edges.params[0].name, "weight");
        assert_eq!(edges.params[0].default, 0.5);

        // Materialize: 2 nodes, 2x2 all-to-all edges.
        let runtime = manifest.materialize();
        assert_eq!(runtime.num_nodes(), 2);
    }

    #[test]
    fn graph_update_predicate_fires_through_vm_and_engine() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::Timebase;

        // One self-looping node (weight 1) seeded with energy 2; its :update fires when
        // energy >= threshold, evaluated on the real scheduler VM. Exercises node-state
        // / node-param accessors + truthiness -> fire through GraphRuntime::process_block.
        let manifest = GraphManifest {
            id: 99,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 2.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                update_source: Some(
                    "(>= (node-state self :energy) (node-param self :threshold))".into(),
                ),
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        // Seeded energy fires at beat 1; the self-loop re-fires every quarter after.
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(samples, vec![48_000, 96_000, 144_000, 192_000]);
        assert!(out.iter().all(|e| e.node_index == 0));
        assert_eq!(scratch.graph_updates.len(), 1);
        assert_eq!(
            scratch
                .graph_updates
                .get(&manifest.id)
                .map(|update| update.source.as_str()),
            manifest.node.update_source.as_deref()
        );
    }

    #[test]
    fn graph_update_reads_input_event_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        // A self-looping node seeded (track 0, note 4) whose :update fires only when the
        // arrived event's note exceeds 1 — exercising node-input-event/event-note on the
        // real scratch VM. The carried note (4) re-emits each hop.
        let manifest = GraphManifest {
            id: 7,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some("(>= (event-note (node-input-event self)) 1)".into()),
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 4.0,
                velocity: 1.0,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        // The seed reaches the node at beat 1 and the self-loop re-fires every quarter;
        // each emission carries the relayed note (4, node transpose 0).
        assert!(!out.is_empty());
        assert!(out.iter().all(|e| e.event.resolved.transpose == 4.0));
    }

    #[test]
    fn graph_update_emit_shapes_velocity_per_hop_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        // A self-looping node seeded (note 10, vel 1.0). Its :update emits via the terse
        // self-less surface, relaying the note unchanged and halving velocity each hop.
        // Because the emitted payload is what rides the scatter, the decayed velocity
        // feeds the next boundary's `in-vel` — proving per-hop velocity accumulation is
        // expressible purely in the DSL (the velocity analogue of the transpose cascade).
        let manifest = GraphManifest {
            id: 31,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some("(emit :note (in-note) :vel (* (in-vel) 0.5))".into()),
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 10.0,
                velocity: 1.0,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert!(out.len() >= 3, "expected several hops, got {}", out.len());
        // Note relays unchanged (emit named :note (in-note), no transpose applied).
        assert!(out.iter().all(|e| e.event.resolved.transpose == 10.0));
        // Velocity halves each hop and the decayed value propagates: 0.5, 0.25, 0.125, …
        let vels: Vec<f32> = out.iter().map(|e| e.event.resolved.velocity).collect();
        assert_eq!(vels[0], 0.5);
        assert_eq!(vels[1], 0.25);
        assert_eq!(vels[2], 0.125);
    }

    #[test]
    fn graph_update_dampen_and_recover_incoming_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphEdge, GraphManifest, GraphPayload, NodeProto, ParamSpec, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = GraphManifest {
            id: 17,
            name: "g".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                params: vec![
                    ParamSpec {
                        name: "threshold".into(),
                        min: 0.0,
                        max: 4.0,
                        default: 1.0,
                        is_int: false,
                    },
                    ParamSpec {
                        name: "dampening".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        is_int: false,
                    },
                    ParamSpec {
                        name: "recovery".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        is_int: false,
                    },
                ],
                update_source: Some(
                    "(if (>= (node-state self :energy) (node-param self :threshold)) (do (dampen-incoming self (node-param self :dampening)) true) (do (recover-incoming self (node-param self :recovery)) false))"
                        .into(),
                ),
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut seed_node = crate::graph::GraphNode {
            resolution: Timebase::Quarter,
            ..crate::graph::GraphNode::default()
        };
        seed_node.seed_track_mask = crate::graph::seed_track_mask(&[0]);
        let param_defaults = manifest
            .node
            .params
            .iter()
            .map(|param| (param.name.clone(), param.default))
            .collect::<HashMap<_, _>>();
        let mut runtime = crate::graph::GraphRuntime::new_with_config(
            17,
            "g".into(),
            vec![
                seed_node,
                crate::graph::GraphNode {
                    resolution: Timebase::Quarter,
                    ..crate::graph::GraphNode::default()
                },
            ],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
            0,
            NeuralMaxPolySelection::Deterministic,
            vec![param_defaults.clone(), param_defaults],
        );
        runtime.seed(0, 0.0, GraphPayload::default());
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(runtime.edge_dampening(0), Some(0.5));

        runtime.process_block(
            1.0,
            2.0,
            48_000,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert_eq!(runtime.edge_dampening(0), Some(0.25));
    }

    fn register_graph_def_sequencer_test_native(runtime: &mut Runtime, state: Arc<SequencerState>) {
        runtime.register_native("def-sequencer", move |args, _ctx| {
            let name = match args.first() {
                Some(Value::String(s) | Value::Symbol(s) | Value::Keyword(s)) => {
                    s.trim_start_matches('@').to_string()
                }
                _ => return Err("def-sequencer expects a name".to_string()),
            };
            if !super::graph_mode_present(&args) {
                return Err("test def-sequencer native only supports graph mode".to_string());
            }
            let manifest = super::parse_graph_manifest(&args)?;
            state.publish_sequencer(PublishedSequencer {
                id: manifest.id,
                name: name.clone(),
                resolution: Timebase::Sixteenth as u8,
                tick_source: String::new(),
                graph: Some(manifest),
            });
            Ok(Value::String(name))
        });
    }

    #[test]
    fn graph_authoring_buffer_overrides_routes_and_emits_multiple_tracks() {
        use crate::graph::GraphPayload;

        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut authoring = Runtime::new();
        register_graph_def_sequencer_test_native(&mut authoring, Arc::clone(&state));
        register_graph_authoring_natives(&mut authoring, Arc::clone(&state));

        authoring
            .eval_str(
                r#"
                (def-sequencer "graph-route-e2e"
                  :shape (line 4)
                  :energy-decay 1
                  :reset-every 0
                  :seed-on-reset 0
                  :max-poly 4
                  :max-poly-selection :deterministic

                  (def-node nrn
                    :resolution :16
                    :delay 1
                    :quantize :16
                    :route 0
                    :seed-from ()
                    :reduce :sum
                    :params ((threshold :float 0 4 :default 0.5)
                             (transpose :int -48 48 :default 0))
                    :state ((energy :leak (per-step :energy-decay)))
                    :update (>= (node-state self :energy)
                                (node-param self :threshold)))

                  (edges
                    :from nrn
                    :to nrn
                    :topology (all-to-all)
                    :gather (edge :weight)
                    :params ((weight :float -1 1 :default 1))))

                (graph-node "graph-route-e2e" 0 :route 0 :seed-from 0)
                (graph-node "graph-route-e2e" 1 :route 1)
                (graph-node "graph-route-e2e" 2 :route 2)
                (graph-node "graph-route-e2e" 3 :route 3)
                "#,
            )
            .expect("evaluate graph authoring buffer");

        let published = state.published_sequencers();
        let manifest = published
            .iter()
            .find_map(|published| published.graph.clone())
            .expect("published graph manifest");
        let graph_overrides = state.current_graph_overrides();
        let overrides = graph_overrides
            .iter()
            .find(|overrides| overrides.sequencer_name == manifest.name)
            .expect("graph overrides");
        assert_eq!(overrides.node_intrinsics.len(), 4);
        assert!(matches!(
            overrides.node_intrinsics[1].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(1))
        ));
        assert!(matches!(
            overrides.node_intrinsics[2].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(2))
        ));
        assert!(matches!(
            overrides.node_intrinsics[3].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(3))
        ));

        let mut graph_runtime = manifest.materialize_with_overrides(Some(overrides));
        graph_runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 1.0,
            },
        );

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        let mut emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut emissions,
        );

        let mut tracks = emissions
            .iter()
            .filter_map(|emission| emission.event.track)
            .collect::<Vec<_>>();
        tracks.sort_unstable();
        tracks.dedup();
        assert_eq!(tracks, vec![0, 1, 2, 3]);
    }

    #[test]
    fn graph_8x8_demo_ui_exposes_node_param_controls_and_weight_matrix() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(def seq-register-step-sequencer-tab (label buffer) nil)")
            .expect("install sequencer tab registration test stub");

        let source = std::fs::read_to_string(format!(
            "{}/scripts/graph-neural-8x8-demo.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read graph 8x8 demo script");
        runtime.eval_str(&source).expect("evaluate graph 8x8 demo");
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the graph demo must publish graph/UI without writing pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published graph manifest");
        assert_eq!(
            manifest.shape.num_nodes(),
            8,
            "the demo matrix must cover every materialized node"
        );

        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("graph 8x8 script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((40.0, 48.0)))
            .expect("graph 8x8 widget tree should lay out");

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected editable weight matrix plus trigger/energy/dampening telemetry"
        );
        for matrix in &matrices {
            assert_measured(matrix);
        }
        let matrix = find_by_stable_key(&layout, "graph-8x8-weight-matrix")
            .expect("weight matrix stable key");
        assert_measured(matrix);
        for key in [
            "graph-8x8-trigger-matrix",
            "graph-8x8-energy-matrix",
            "graph-8x8-dampening-matrix",
        ] {
            let widget = find_by_stable_key(&layout, key)
                .unwrap_or_else(|| panic!("missing telemetry matrix {key}"));
            assert_measured(widget);
        }

        // Five number-pickers per node (delay + transpose + vel-decay + dampening +
        // recovery) plus the two top-of-panel config pickers (reset-bars + max-poly);
        // three dropdowns per node (route + resolution + quantize).
        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            8 * 5 + 2,
            "expected delay/transpose/vel/dampening/recovery per node + reset-bars + max-poly"
        );
        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            24,
            "expected route + resolution + quantize per node"
        );
        for key in ["graph-8x8-reset-bars", "graph-8x8-max-poly"] {
            let widget = find_by_stable_key(&layout, key)
                .unwrap_or_else(|| panic!("missing config control {key}"));
            assert_measured(widget);
        }
        for idx in 0..8 {
            for key in [
                format!("graph-8x8-route-{idx}"),
                format!("graph-8x8-delay-{idx}"),
                format!("graph-8x8-transpose-{idx}"),
                format!("graph-8x8-vel-decay-{idx}"),
                format!("graph-8x8-dampening-{idx}"),
                format!("graph-8x8-recovery-{idx}"),
                format!("graph-8x8-resolution-{idx}"),
                format!("graph-8x8-quantize-{idx}"),
            ] {
                let widget = find_by_stable_key(&layout, &key)
                    .unwrap_or_else(|| panic!("missing control {key}"));
                assert_measured(widget);
            }
        }
        let quantize_options = find_by_stable_key(&layout, "graph-8x8-quantize-0")
            .and_then(|node| node.props.get("options"))
            .expect("quantize options");
        let Value::List(options) = quantize_options else {
            panic!("quantize options should be a list");
        };
        for label in ["2T", "4T", "8T", "16T", "32T", "64T"] {
            assert!(
                options.iter().any(
                    |option| matches!(&*option.borrow(), Value::String(value) if value == label)
                ),
                "missing quantize triplet option {label}"
            );
        }

        runtime
            .eval_str("(g8-init-ring-defaults)")
            .expect("explicitly initialize graph demo defaults");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after explicit init");
        assert_eq!(
            graph.edge_params.len(),
            64,
            "explicit init should write the full ring weight matrix"
        );
        assert!(
            graph.edge_params.iter().any(|edge| {
                edge.from == 0 && edge.to == 1 && edge.param == "weight" && edge.value == 1.0
            }),
            "explicit init should write the first ring edge"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 0
                    && node.seed_from == Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0]))
            }),
            "explicit init should seed node 0 from track 0"
        );

        // transpose picker -> per-node behavioral param override.
        let transpose_change = find_by_stable_key(&layout, "graph-8x8-transpose-2")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("transpose callback");
        runtime
            .invoke(transpose_change, vec![Value::Number(7.0)])
            .expect("invoke transpose callback");
        // vel-decay picker -> per-node behavioral param override (the velocity analogue).
        let vel_change = find_by_stable_key(&layout, "graph-8x8-vel-decay-5")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("vel-decay callback");
        runtime
            .invoke(vel_change, vec![Value::Number(0.5)])
            .expect("invoke vel-decay callback");
        // resolution dropdown -> per-node intrinsic override.
        let resolution_change = find_by_stable_key(&layout, "graph-8x8-resolution-3")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("resolution callback");
        runtime
            .invoke(resolution_change, vec![Value::String("8".into())])
            .expect("invoke resolution callback");
        // route dropdown -> per-node intrinsic route override.
        let route_change = find_by_stable_key(&layout, "graph-8x8-route-4")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("route callback");
        runtime
            .invoke(route_change, vec![Value::String("Track 3".into())])
            .expect("invoke route callback");

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides");
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 2 && param.param == "transpose" && param.value == 7.0
            }),
            "transpose knob should write a node param override"
        );
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 5 && param.param == "vel-decay" && param.value == 0.5
            }),
            "vel-decay knob should write a node param override"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 3
                    && node.resolution == Some(crate::sequencer::Timebase::Eighth as u8)
            }),
            "resolution dropdown should write an intrinsic override"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 4
                    && node.route == Some(crate::graph::ProjectGraphRouteOverride::Track(2))
            }),
            "route dropdown should write an intrinsic override"
        );

        {
            let mut bank = state.pattern.pattern_bank.lock().unwrap();
            let mut pattern = bank[0].clone();
            let graph = pattern
                .graph_overrides
                .iter_mut()
                .find(|graph| graph.sequencer_name == "neural-8x8-demo")
                .expect("cloned graph overrides");
            graph
                .node_params
                .push(crate::graph::ProjectGraphNodeParamOverride {
                    group: "nrn".to_string(),
                    instance: 2,
                    param: "transpose".to_string(),
                    value: -12.0,
                });
            graph
                .node_intrinsics
                .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 3,
                    resolution: None,
                    delay_steps: Some(6),
                    quantize: None,
                    route: None,
                    seed_from: None,
                });
            graph
                .node_intrinsics
                .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 4,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: Some(crate::graph::ProjectGraphRouteOverride::Track(0)),
                    seed_from: None,
                });
            graph
                .edge_params
                .push(crate::graph::ProjectGraphEdgeParamOverride {
                    group: "nrn->nrn".to_string(),
                    from: 0,
                    to: 1,
                    param: "weight".to_string(),
                    value: 0.25,
                });
            bank.push(pattern);
        }
        state.pattern.num_patterns.store(2, Ordering::Relaxed);
        state.pattern.current_pattern.store(1, Ordering::Relaxed);
        runtime.set_reactive("SEQ", "current-pattern", Value::Number(1.0));
        runtime.run_reactive_cycle();
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 2 :transpose))")
                .expect("read bound transpose value"),
            Some(Value::Number(-12.0)),
            "pattern switch should reload transpose control state"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 3 :delay))")
                .expect("read bound delay value"),
            Some(Value::Number(6.0)),
            "pattern switch should reload delay control state"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 4 :route g8-route-options))")
                .expect("read bound route index"),
            Some(Value::Number(0.0)),
            "pattern switch should display internal route 0 as Track 1 (index 0)"
        );
        assert_eq!(
            runtime
                .eval_str("(nth (nth g8-weights 0) 1)")
                .expect("read synced weight"),
            Some(Value::Number(0.25)),
            "pattern switch should reload matrix state"
        );
        state.pattern.current_pattern.store(0, Ordering::Relaxed);
        runtime.set_reactive("SEQ", "current-pattern", Value::Number(0.0));
        runtime.run_reactive_cycle();

        let mut graph_runtime = manifest.materialize_with_overrides(Some(graph));
        assert_eq!(graph_runtime.seed_track_mask_for_node(0), Some(1));
        let seeded = graph_runtime.seed(
            0,
            0.0,
            crate::graph::GraphPayload {
                note: 0.0,
                velocity: 1.0,
            },
        );
        assert_eq!(seeded, 1, "track 0 should seed node 0 exactly once");
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(8),
            fallback_instrument_descriptors(8),
            0,
            0,
        );
        let mut chunked_emissions = Vec::new();
        let mut start_beats = 0.0_f64;
        let mut eval_count = 0_usize;
        let mut max_input = 0.0_f64;
        let mut max_energy = 0.0_f64;
        while start_beats < 1.0 {
            let end_beats = (start_beats + 0.021_f64).min(1.0_f64);
            graph_runtime.process_block(
                start_beats,
                end_beats,
                0,
                48_000.0,
                manifest.max_poly,
                |eval| {
                    eval_count += 1;
                    max_input = max_input.max(eval.input);
                    max_energy = max_energy.max(eval.energy);
                    scratch
                        .invoke_graph_update(&manifest, eval)
                        .expect("demo graph update should evaluate")
                },
                &mut chunked_emissions,
            );
            start_beats = end_beats;
        }
        assert!(eval_count > 0, "chunked graph drive should evaluate nodes");
        assert!(
            !chunked_emissions.is_empty(),
            "track-0 seed should propagate through the ring under chunked graph drive; evals={eval_count} max_input={max_input} max_energy={max_energy} edge_overrides={}",
            graph.edge_params.len()
        );

        let matrix_cell_change = matrix
            .props
            .get("on-cell-change")
            .cloned()
            .expect("matrix cell callback");
        // A single cell edit writes exactly one edge override; (from=3, to=4) == 0.5.
        runtime
            .invoke(
                matrix_cell_change.clone(),
                vec![gv_num(3.0), gv_num(4.0), gv_num(0.5)],
            )
            .expect("invoke matrix cell callback");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after matrix edit");
        assert_eq!(graph.edge_params.len(), 64);
        assert!(graph.edge_params.iter().any(|edge| {
            edge.from == 3 && edge.to == 4 && edge.param == "weight" && edge.value == 0.5
        }));

        // Zero every weight one cell at a time (the per-cell edit path) to silence the net.
        for r in 0..8 {
            for c in 0..8 {
                runtime
                    .invoke(
                        matrix_cell_change.clone(),
                        vec![gv_num(r as f64), gv_num(c as f64), gv_num(0.0)],
                    )
                    .expect("invoke zero matrix cell callback");
            }
        }

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after zero matrix edit");
        let mut graph_runtime = manifest.materialize_with_overrides(Some(graph));
        graph_runtime.seed(
            0,
            0.0,
            crate::graph::GraphPayload {
                note: 0.0,
                velocity: 1.0,
            },
        );
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(8),
            fallback_instrument_descriptors(8),
            0,
            0,
        );
        let mut emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut emissions,
        );
        assert!(
            emissions.is_empty(),
            "zero matrix should silence graph propagation"
        );
    }

    #[test]
    fn graph_8x8_demo_scratch_load_preserves_saved_overrides() {
        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let expected = crate::graph::ProjectGraphOverrides {
            sequencer_id: super::stable_sequencer_id("neural-8x8-demo"),
            sequencer_name: "neural-8x8-demo".to_string(),
            node_intrinsics: vec![
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 0,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: None,
                    seed_from: Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0])),
                },
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 3,
                    resolution: None,
                    delay_steps: Some(6),
                    quantize: None,
                    route: None,
                    seed_from: None,
                },
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 4,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: Some(crate::graph::ProjectGraphRouteOverride::Track(0)),
                    seed_from: None,
                },
            ],
            node_params: vec![crate::graph::ProjectGraphNodeParamOverride {
                group: "nrn".to_string(),
                instance: 2,
                param: "transpose".to_string(),
                value: -12.0,
            }],
            edge_params: vec![crate::graph::ProjectGraphEdgeParamOverride {
                group: "nrn->nrn".to_string(),
                from: 0,
                to: 1,
                param: "weight".to_string(),
                value: 0.25,
            }],
            reset_every_beats: None,
            max_poly: None,
        };
        {
            let mut bank = state.pattern.pattern_bank.lock().unwrap();
            bank[0].graph_overrides = vec![expected.clone()];
        }

        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|crates_dir| crates_dir.parent())
            .expect("sequencer crate should live under workspace crates dir")
            .join(".eseqlisp-scratch");
        let report = runtime.eval_source_transactional(
            Some(workspace_root),
            r#"
            (def seq-register-step-sequencer-tab (label buffer) nil)
            (load "crates/sequencer/scripts/graph-neural-8x8-demo.lisp")
            "#,
            Vec::new(),
        );
        assert!(
            report.success,
            "scratch-style load failed: {}",
            report.failure_message()
        );
        assert_eq!(
            runtime.eval_str("g8-name").expect("read loaded graph name"),
            Some(Value::String("neural-8x8-demo".to_string())),
            "scratch-style load should define the graph demo UI state"
        );

        assert!(
            state
                .published_sequencers()
                .into_iter()
                .any(|published| published.name == "neural-8x8-demo" && published.graph.is_some()),
            "scratch load should republish the graph manifest"
        );
        assert_eq!(
            state.current_graph_overrides(),
            vec![expected],
            "scratch load must not clobber saved graph overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 2 :transpose))")
                .expect("read bound transpose value"),
            Some(Value::Number(-12.0)),
            "loaded UI should sync node params from saved overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 3 :delay))")
                .expect("read bound delay value"),
            Some(Value::Number(6.0)),
            "loaded UI should sync node intrinsics from saved overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 4 :route g8-route-options))")
                .expect("read bound route index"),
            Some(Value::Number(0.0)),
            "loaded UI should display saved internal route 0 as Track 1 (index 0)"
        );
        assert_eq!(
            runtime
                .eval_str("(nth (nth g8-weights 0) 1)")
                .expect("read synced weight"),
            Some(Value::Number(0.25)),
            "loaded UI should sync matrix weights from saved overrides"
        );
    }

    #[test]
    fn graph_authoring_natives_write_current_pattern_overrides() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 123,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 2,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(graph-node \"neural\" 1 :delay 3 :route 0 :seed-from 0)")
            .expect("graph-node");
        runtime
            .eval_str("(graph-param \"neural\" 1 :threshold 0.75)")
            .expect("graph-param");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :weight 0.5)")
            .expect("graph-edge");

        let overrides = state.current_graph_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].node_intrinsics[0].delay_steps, Some(3));
        assert_eq!(overrides[0].node_params[0].value, 0.75);
        assert_eq!(overrides[0].edge_params[0].value, 0.5);
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :delay)")
                .expect("graph-node-value delay"),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :route)")
                .expect("graph-node-value route"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-param-value \"neural\" 1 :threshold)")
                .expect("graph-param-value threshold"),
            Some(Value::Number(0.75))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"neural\" :from 0 :to 1 :weight)")
                .expect("graph-edge-value keyword syntax"),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"neural\" 0 1 :weight)")
                .expect("graph-edge-value positional syntax"),
            Some(Value::Number(0.5))
        );
    }

    #[test]
    fn bind_graph_seeds_reactive_slots_and_keys_round_trip() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 77,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 2,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "transpose".into(),
                    min: -48.0,
                    max: 48.0,
                    default: 0.0,
                    is_int: true,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(graph-node \"neural\" 1 :delay 4 :route 2)")
            .expect("graph-node");
        runtime
            .eval_str("(graph-param \"neural\" 1 :transpose -7)")
            .expect("graph-param");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :weight 0.5)")
            .expect("graph-edge");

        // Numeric intrinsic + param bind-graph handles read the resolved value from the slot.
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 1 :delay))")
                .expect("bind-graph delay"),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 1 :transpose))")
                .expect("bind-graph transpose"),
            Some(Value::Number(-7.0))
        );
        // Enum intrinsic binds to the dropdown index within the supplied options
        // (route 2 -> "Track 3" -> index 2).
        assert_eq!(
            runtime
                .eval_str(
                    "(reactive-value (bind-graph \"neural\" 1 :route \
                     (list \"Track 1\" \"Track 2\" \"Track 3\" \"Off\")))"
                )
                .expect("bind-graph route index"),
            Some(Value::Number(2.0))
        );
        // Edge handle.
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-edge \"neural\" 0 1 :weight))")
                .expect("bind-graph-edge weight"),
            Some(Value::Number(0.5))
        );

        // graph-key / graph-edge-key name the exact slot a reactive-set dirties, so a
        // plain `bind` to that key observes the new value (this is the edit-writeback path).
        runtime
            .eval_str("(reactive-set \"GRAPH\" (graph-key \"neural\" 1 :delay) 9)")
            .expect("reactive-set node key");
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind \"GRAPH\" (graph-key \"neural\" 1 :delay)))")
                .expect("read node slot"),
            Some(Value::Number(9.0))
        );
        runtime
            .eval_str("(reactive-set \"GRAPH\" (graph-edge-key \"neural\" 0 1 :weight) 0.2)")
            .expect("reactive-set edge key");
        assert_eq!(
            runtime
                .eval_str(
                    "(reactive-value (bind \"GRAPH\" (graph-edge-key \"neural\" 0 1 :weight)))"
                )
                .expect("read edge slot"),
            Some(Value::Number(0.2))
        );
    }

    #[test]
    fn graph_config_overrides_round_trip_and_reach_runtime() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 88,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 16.0, // 4 bars
            seed_on_reset: 0.0,
            max_poly: 4,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest.clone()),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));

        // Manifest defaults reported in UI units (16 beats / 4 = 4 bars; cap 4).
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :reset-bars)")
                .expect("reset-bars default"),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :max-poly)")
                .expect("max-poly default"),
            Some(Value::Number(4.0))
        );

        // Override both; reset-bars persists as beats (2 bars -> 8 beats).
        runtime
            .eval_str("(graph-config \"neural\" :reset-bars 2)")
            .expect("set reset-bars");
        runtime
            .eval_str("(graph-config \"neural\" :max-poly 1)")
            .expect("set max-poly");
        let overrides = state.current_graph_overrides();
        assert_eq!(overrides[0].reset_every_beats, Some(8.0));
        assert_eq!(overrides[0].max_poly, Some(1));
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :reset-bars)")
                .expect("reset-bars override"),
            Some(Value::Number(2.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-config \"neural\" :max-poly))")
                .expect("bound max-poly"),
            Some(Value::Number(1.0))
        );

        // The overrides actually reach the materialized runtime config.
        let config = manifest.runtime_config_with_overrides(Some(&overrides[0]));
        assert_eq!(config.reset_interval_beats, 8.0);
        assert_eq!(config.max_poly, 1);
    }

    #[test]
    fn parse_graph_manifest_requires_shape_and_node() {
        let no_shape = vec![gv_sym("g"), gv_list(vec![gv_sym("def-node"), gv_sym("n")])];
        assert!(super::parse_graph_manifest(&no_shape)
            .unwrap_err()
            .contains(":shape"));

        let no_node = vec![
            gv_sym("g"),
            gv_kw("shape"),
            gv_list(vec![gv_sym("grid"), gv_num(2.0), gv_num(2.0)]),
        ];
        assert!(super::parse_graph_manifest(&no_node)
            .unwrap_err()
            .contains("def-node"));
    }

    static CAPTURED_DGEN_SAMPLE_RATE_BITS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn capture_dgen_sample_rate_process(
        _inp: *const *mut f32,
        _out: *const *mut f32,
        _nframes: std::os::raw::c_int,
        _state: *mut std::ffi::c_void,
        _buffers: *mut std::ffi::c_void,
        host_sample_rate: std::os::raw::c_float,
    ) {
        CAPTURED_DGEN_SAMPLE_RATE_BITS.store(host_sample_rate.to_bits(), Ordering::SeqCst);
    }

    fn descriptors_with_filter(track_count: usize) -> Vec<Vec<EffectDescriptor>> {
        (0..track_count)
            .map(|_| {
                let mut chain = EffectDescriptor::default_full_chain();
                chain[0] = EffectDescriptor::builtin_filter();
                chain
            })
            .collect()
    }

    fn bind_filter_slot(state: &SequencerState) {
        state.pattern.effect_chains[0][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
    }

    #[test]
    fn folder_instrument_dsp_path_maps_to_instrument_name() {
        let path =
            std::path::Path::new("instruments/monomachine/fmplus/monomachine-fmplus/dsp.lisp");
        assert_eq!(
            super::instrument_name_from_source_path(path).as_deref(),
            Some("monomachine/fmplus/monomachine-fmplus/")
        );
        assert_eq!(
            super::source_name_from_path(&eseqlisp::CompileKind::Instrument, path).as_deref(),
            Some("monomachine/fmplus/monomachine-fmplus/")
        );
    }

    #[test]
    fn parse_manifest_uses_dgen_param_span_metadata() {
        let manifest = parse_manifest(
            r#"{
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [
                    {"name": "implicit_scalar", "cellId": 2, "default": 0.1},
                    {"name": "scalar", "cellId": 4, "cellSpan": 1, "default": 0.25},
                    {"name": "vector", "cellId": 8, "vectorWidth": 4, "default": 0.5}
                ]
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.params[0].cell_span, 1);
        assert_eq!(manifest.params[1].cell_span, 1);
        assert_eq!(manifest.params[2].cell_span, 4);
        assert!(manifest.params.iter().all(|param| param.group.is_none()));
        assert!(manifest.params.iter().all(|param| param.env.is_none()));
        assert!(manifest.params.iter().all(|param| param.role.is_none()));
        assert!(manifest.groups.is_empty());
        assert!(manifest.envelopes.is_empty());
    }

    #[test]
    fn parse_manifest_reads_ui_metadata() {
        let manifest = parse_manifest(
            r#"{
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [
                    {
                        "name": "amp_attack",
                        "cellId": 2,
                        "default": 0.01,
                        "min": 0,
                        "max": 2,
                        "group": "amp",
                        "env": "amp_env",
                        "role": "attack"
                    },
                    {
                        "name": "cutoff",
                        "cellId": 3,
                        "default": 1000,
                        "min": 20,
                        "max": 20000,
                        "group": "filter"
                    }
                ],
                "groups": [
                    { "name": "amp" },
                    { "name": "filter" }
                ],
                "envelopes": [
                    {
                        "name": "amp_env",
                        "group": "amp",
                        "roles": {
                            "attack": "amp_attack",
                            "decay": "amp_decay",
                            "sustain": "amp_sustain",
                            "release": "amp_release"
                        }
                    }
                ]
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.params[0].group.as_deref(), Some("amp"));
        assert_eq!(manifest.params[0].env.as_deref(), Some("amp_env"));
        assert_eq!(manifest.params[0].role.as_deref(), Some("attack"));
        assert_eq!(manifest.params[1].group.as_deref(), Some("filter"));
        assert_eq!(manifest.params[1].env, None);
        assert_eq!(
            manifest
                .groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amp", "filter"]
        );
        assert_eq!(manifest.envelopes.len(), 1);
        assert_eq!(manifest.envelopes[0].name, "amp_env");
        assert_eq!(manifest.envelopes[0].group.as_deref(), Some("amp"));
        assert_eq!(
            manifest.envelopes[0].roles.attack.as_deref(),
            Some("amp_attack")
        );
        assert_eq!(
            manifest.envelopes[0].roles.release.as_deref(),
            Some("amp_release")
        );
    }

    #[test]
    fn dgen_init_message_honors_param_span() {
        let manifest = super::DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 2,
            process_abi: "dgen-c-v2-host-sample-rate".to_string(),
            total_memory_slots: 16,
            params: vec![
                DGenParam {
                    name: "scalar".to_string(),
                    cell_id: 4,
                    cell_span: 1,
                    default: 0.25,
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                    hidden: false,
                    group: None,
                    env: None,
                    role: None,
                },
                DGenParam {
                    name: "vector".to_string(),
                    cell_id: 8,
                    cell_span: 4,
                    default: 0.5,
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                    hidden: false,
                    group: None,
                    env: None,
                    role: None,
                },
            ],
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 2,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        };

        let init = super::build_init_message_for_voice(0, &manifest, 0);
        let entries = init[6..]
            .chunks_exact(2)
            .map(|entry| (entry[0] as usize, entry[1]))
            .collect::<Vec<_>>();

        assert!(entries.contains(&(4, 0.25)));
        assert!(!entries.contains(&(5, 0.25)));
        assert!(entries.contains(&(8, 0.5)));
        assert!(entries.contains(&(11, 0.5)));
    }

    #[test]
    fn dgenlisp_init_writes_host_sample_rate_without_shifting_compact_entries() {
        let total_memory_slots = 16;
        let mut state = vec![0.0_f32; super::dgen_total_state_slots(total_memory_slots)];
        let initial_state = [
            7.0,
            total_memory_slots as f32,
            super::HEADER_CANARY,
            2.0,
            1.0,
            2.0,
            4.0,
            0.25,
            8.0,
            0.5,
        ];

        unsafe {
            super::dgenlisp_init(
                state.as_mut_ptr() as *mut std::ffi::c_void,
                48_000,
                128,
                initial_state.as_ptr() as *const std::ffi::c_void,
            );
        }

        assert_eq!(state[super::DGEN_HOST_SAMPLE_RATE_IDX], 48_000.0);
        assert_eq!(state[super::HEADER_SLOTS + 4], 0.25);
        assert_eq!(state[super::HEADER_SLOTS + 8], 0.5);
        let write_offset = super::HEADER_SLOTS + super::dgen_buffer_span_slots(total_memory_slots);
        assert_eq!(state[write_offset + 4], 0.25);
        assert_eq!(state[write_offset + 8], 0.5);
    }

    #[test]
    fn dgenlisp_wrapper_passes_header_sample_rate_to_generated_process() {
        let total_memory_slots = 4;
        let slot_id = 17usize;
        let mut state = vec![0.0_f32; super::dgen_total_state_slots(total_memory_slots)];
        state[0] = slot_id as f32;
        state[1] = total_memory_slots as f32;
        state[2] = super::HEADER_CANARY;
        state[3] = 1.0;
        state[super::DGEN_ENABLED_PARAM_IDX] = 1.0;
        state[super::DGEN_HOST_SAMPLE_RATE_IDX] = 48_000.0;

        CAPTURED_DGEN_SAMPLE_RATE_BITS.store(0, Ordering::SeqCst);
        super::set_dgen_process_fn(slot_id, capture_dgen_sample_rate_process);

        let mut input = vec![0.0_f32; 8];
        let mut output = vec![0.0_f32; 8];
        let inputs = [input.as_mut_ptr()];
        let outputs = [output.as_mut_ptr()];
        unsafe {
            super::dgenlisp_wrapper_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                8,
                state.as_mut_ptr() as *mut std::ffi::c_void,
                std::ptr::null_mut(),
            );
        }
        super::set_dgen_process_fn_raw(slot_id, 0);

        assert_eq!(
            f32::from_bits(CAPTURED_DGEN_SAMPLE_RATE_BITS.load(Ordering::SeqCst)),
            48_000.0
        );
    }

    #[test]
    fn parse_manifest_reads_process_abi_and_tensor_source_sample_rate() {
        let manifest = parse_manifest(
            r#"{
                "version": 2,
                "processAbi": "dgen-c-v2-host-sample-rate",
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [],
                "tensors": [
                    {
                        "name": "sample",
                        "cellOffset": 4,
                        "shape": [8],
                        "kind": "audio",
                        "mutable": false,
                        "sourceFile": "sample.wav",
                        "sourceSampleRate": 48000
                    }
                ],
                "tensorInitData": []
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.version, 2);
        assert_eq!(manifest.process_abi, "dgen-c-v2-host-sample-rate");
        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].source_sample_rate, Some(48_000));
    }

    #[test]
    fn built_in_instrument_dsp_files_do_not_hardcode_44100_sample_rate() {
        fn visit(dir: &std::path::Path, failures: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, failures);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("lisp") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (idx, line) in source.lines().enumerate() {
                    let code = line.split(';').next().unwrap_or("");
                    if code.contains("44100") || code.contains("44.1") {
                        failures.push(format!("{}:{}", path.display(), idx + 1));
                    }
                }
            }
        }

        let mut failures = Vec::new();
        visit(std::path::Path::new(super::INSTRUMENTS_DIR), &mut failures);

        assert!(
            failures.is_empty(),
            "hardcoded 44.1kHz timing constants found in Lisp DSP files:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn parse_manifest_reads_mod_active_flag_and_depth_lanes() {
        let json = r#"
        {
          "totalMemorySlots": 128,
          "params": [
            { "name": "gain", "cellId": 10, "default": 0.5, "min": 0, "max": 1 }
          ],
          "inputs": [],
          "outputs": [],
          "modulators": [
            { "slot": 1, "inputChannel": 4, "name": "mod1" },
            { "slot": 2, "inputChannel": 5, "name": "mod2" }
          ],
          "modDestinations": [
            {
              "name": "gain",
              "paramCellId": 10,
              "activeCellId": 20,
              "depthLanes": [
                { "slot": 1, "depthCellId": 21 },
                { "slot": 2, "depthCellId": 22 }
              ],
              "mode": "additive",
              "min": 0,
              "max": 1
            }
          ],
          "tensors": [],
          "tensorInitData": []
        }
        "#;

        let manifest = parse_manifest(json).expect("manifest parses");
        assert_eq!(manifest.mod_destinations.len(), 1);
        let dest = &manifest.mod_destinations[0];
        assert_eq!(dest.active_cell_id, 20);
        assert_eq!(
            dest.depth_lanes
                .iter()
                .map(|lane| (lane.slot, lane.depth_cell_id))
                .collect::<Vec<_>>(),
            vec![(1, 21), (2, 22)]
        );
    }

    #[test]
    fn parse_manifest_reads_modulation_outputs() {
        let json = r#"
        {
          "totalMemorySlots": 128,
          "inputs": [],
          "outputs": [{ "channel": 0, "name": "audio" }],
          "modOutputs": [
            { "slot": 1, "channel": 2, "name": "macro-a", "range": "unipolar" },
            { "slot": 2, "channel": 3, "name": "macro-b", "range": "unipolar" }
          ],
          "tensors": [],
          "tensorInitData": []
        }
        "#;

        let manifest = parse_manifest(json).expect("manifest parses");
        assert_eq!(manifest.n_outputs, 1);
        assert_eq!(manifest.mod_outputs.len(), 2);
        assert_eq!(
            manifest
                .mod_outputs
                .iter()
                .map(|output| (
                    output.slot,
                    output.channel,
                    output.name.as_str(),
                    output.range.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![(1, 2, "macro-a", "unipolar"), (2, 3, "macro-b", "unipolar")]
        );
    }

    #[test]
    fn parse_manifest_defaults_missing_modulation_outputs_to_empty() {
        let manifest = parse_manifest(
            r#"{
                "totalMemorySlots": 16,
                "inputs": [],
                "outputs": [{ "channel": 0, "name": "audio" }]
            }"#,
        )
        .expect("manifest parses");

        assert!(manifest.mod_outputs.is_empty());
        assert_eq!(manifest.n_outputs, 1);
    }

    #[test]
    fn effect_host_modulation_controls_use_effect_local_bank() {
        let manifest = parse_manifest(
            r#"
            {
              "totalMemorySlots": 128,
              "params": [
                { "name": "gain", "cellId": 10, "default": 0.5, "min": 0, "max": 1 }
              ],
              "inputs": [],
              "outputs": [{ "channel": 0, "name": "out" }],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "mod1" },
                { "slot": 2, "inputChannel": 3, "name": "mod2" }
              ],
              "modDestinations": [
                {
                  "name": "gain",
                  "paramCellId": 10,
                  "activeCellId": 20,
                  "depthLanes": [
                    { "slot": 1, "depthCellId": 21 },
                    { "slot": 2, "depthCellId": 22 }
                  ],
                  "mode": "additive",
                  "min": 0,
                  "max": 1
                }
              ],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        let mut desc = EffectDescriptor::from_lisp_manifest(
            "MODDED_GAIN",
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        super::append_effect_host_modulation_controls(&mut desc, &manifest);

        assert!(super::effect_has_host_modulation(&manifest));
        assert_eq!(
            desc.instrument_modulators.len(),
            crate::voice_modulator::SLOT_COUNT
        );
        let mod1_source = desc
            .params
            .iter()
            .find(|param| param.name == "mod1_source")
            .expect("effect descriptor should expose Mod 1 source");
        assert_eq!(mod1_source.default, 0.0);
        assert!(mod1_source.node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE);

        let depth = desc
            .params
            .iter()
            .find(|param| param.name == "mod gain slot 1 amt")
            .expect("effect descriptor should expose DGen depth param");
        assert_eq!(depth.node_param_idx, (super::HEADER_SLOTS + 21) as u32);
        assert_eq!(
            desc.instrument_modulation_targets
                .iter()
                .map(|target| {
                    (
                        desc.params[target.base_param_idx].name.as_str(),
                        target.modulator_slot,
                        desc.params[target.depth_param_idx].name.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("gain", 1, "mod gain slot 1 amt"),
                ("gain", 2, "mod gain slot 2 amt"),
            ]
        );
    }

    #[test]
    fn legacy_effect_modulator_inputs_are_sidechain_controls_without_host_modulation() {
        let manifest = parse_manifest(
            r#"
            {
              "totalMemorySlots": 128,
              "params": [],
              "inputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" },
                { "channel": 2, "name": "signal" }
              ],
              "outputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" }
              ],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "signal" }
              ],
              "modDestinations": [],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        assert_eq!(
            effect_sidechain_inputs(&manifest),
            vec![DGenSidechainInput {
                input_channel: 2,
                name: "sidechain signal".to_string(),
            }]
        );
    }

    #[test]
    fn effect_host_modulation_can_coexist_with_named_sidechain_input() {
        let manifest = parse_manifest(
            r#"
            {
              "totalMemorySlots": 128,
              "params": [
                { "name": "threshold", "cellId": 10, "default": -20, "min": -80, "max": -2 }
              ],
              "inputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" },
                { "channel": 2, "name": "mod1" },
                { "channel": 3, "name": "mod2" },
                { "channel": 4, "name": "mod3" },
                { "channel": 5, "name": "mod4" },
                { "channel": 6, "name": "sidechain" }
              ],
              "outputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" }
              ],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "mod1" },
                { "slot": 2, "inputChannel": 3, "name": "mod2" },
                { "slot": 3, "inputChannel": 4, "name": "mod3" },
                { "slot": 4, "inputChannel": 5, "name": "mod4" }
              ],
              "modDestinations": [
                {
                  "name": "threshold",
                  "paramCellId": 10,
                  "activeCellId": 20,
                  "depthLanes": [
                    { "slot": 1, "depthCellId": 21 }
                  ],
                  "mode": "additive",
                  "min": -80,
                  "max": -2
                }
              ],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        assert!(effect_has_host_modulation(&manifest));
        assert_eq!(
            effect_sidechain_inputs(&manifest),
            vec![DGenSidechainInput {
                input_channel: 6,
                name: "sidechain".to_string(),
            }]
        );
    }

    use std::sync::Arc;

    fn neural_test_runtime(track_count: usize) -> (Arc<SequencerState>, Runtime) {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("neural-networks", Value::List(Vec::new())),
                ("neural-energy-matrix", Value::List(Vec::new())),
                ("neural-trigger-matrix", Value::List(Vec::new())),
                ("neural-dampening-matrix", Value::List(Vec::new())),
                ("selected-neural-neurons", Value::List(Vec::new())),
            ],
            true,
        );
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(track_count),
                fallback_instrument_descriptors(track_count),
            ),
        );
        (state, runtime)
    }

    #[test]
    fn neural_lisp_create_list_describe_delete() {
        let (state, mut runtime) = neural_test_runtime(1);

        let created = runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();
        assert!(matches!(created, Some(Value::Map(_))));

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].id, 1);
        assert_eq!(networks[0].name, "drums");
        assert_eq!(networks[0].num_neurons, 3);
        assert_eq!(networks[0].neurons.len(), 3);
        assert_eq!(networks[0].weights, vec![vec![0.0; 3]; 3]);

        let listed = runtime.eval_str("(neural-list)").unwrap();
        match listed {
            Some(Value::List(items)) => assert_eq!(items.len(), 1),
            other => panic!("expected neural-list to return list, got {other:?}"),
        }

        let described = runtime.eval_str("(neural-describe \"drums\")").unwrap();
        assert!(matches!(described, Some(Value::Map(_))));

        let deleted = runtime.eval_str("(neural-delete \"drums\")").unwrap();
        assert_eq!(deleted, Some(Value::Bool(true)));
        assert!(state.current_neural_networks().is_empty());
    }

    #[test]
    fn neural_lisp_enable_set_and_neuron_edit() {
        let (state, mut runtime) = neural_test_runtime(2);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 2 :enabled false)")
            .unwrap();

        let enabled = runtime.eval_str("(neural-enable \"drums\" true)").unwrap();
        assert!(matches!(enabled, Some(Value::Map(_))));

        runtime
            .eval_str(
                "(neural-set \"drums\" :reset-bars 2 :energy-decay 0.5 :max-poly 4 :max-poly-selection :random :name \"kit\")",
            )
            .unwrap();
        runtime
            .eval_str(
                "(neural-neuron \"kit\" 1 :route 1 :resolution :8 :threshold 0.75 :delay 3 :quantize :16 :transpose -12 :dampening 0.2 :recovery 0.9)",
            )
            .unwrap();

        let networks = state.current_neural_networks();
        let network = &networks[0];
        assert_eq!(network.name, "kit");
        assert!(network.enabled);
        assert_eq!(network.reset_interval_bars, 2.0);
        assert_eq!(network.energy_decay, 0.5);
        assert_eq!(network.max_poly, 4);
        assert_eq!(network.max_poly_selection, NeuralMaxPolySelection::Random);

        let neuron = &network.neurons[1];
        assert_eq!(neuron.route, Some(1));
        assert_eq!(
            neuron.resolution_timebase(),
            crate::sequencer::Timebase::Eighth
        );
        assert_eq!(neuron.threshold, 0.75);
        assert_eq!(neuron.delay_steps, 3);
        assert_eq!(
            neuron.quantize_timebase(),
            Some(crate::sequencer::Timebase::Sixteenth)
        );
        assert_eq!(neuron.transpose, -12.0);
        assert_eq!(neuron.dampening_amount, 0.2);
        assert_eq!(neuron.dampening_recovery, 0.9);

        runtime
            .eval_str("(neural-set \"kit\" :max-poly-selection :propagation)")
            .unwrap();
        assert_eq!(
            state.current_neural_networks()[0].max_poly_selection,
            NeuralMaxPolySelection::Propagation
        );
    }

    #[test]
    fn neural_lisp_network_edits_do_not_bump_pattern_epoch() {
        let (state, mut runtime) = neural_test_runtime(2);
        let created = runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap()
            .unwrap();
        let Value::Map(created) = created else {
            panic!("expected created network map");
        };
        let id = match created.get("id").map(|value| value.borrow().clone()) {
            Some(Value::Number(id)) => id as u64,
            other => panic!("expected created network id, got {other:?}"),
        };
        let epoch_before = state.transport.pattern_epoch.load(Ordering::Relaxed);

        runtime
            .eval_str(&format!(
                "(neural-neuron {id} 1 :route 1 :delay 3 :dampening 0.5)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!("(neural-weight {id} :from 0 :to 1 :value 0.75)"))
            .unwrap();
        runtime
            .eval_str(&format!("(neural-set {id} :reset-bars 2 :max-poly 4)"))
            .unwrap();

        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before,
            "neural network authoring should publish a scheduler snapshot without forcing a pattern-epoch reset"
        );
    }

    #[test]
    fn neural_lisp_weights_matrix_and_single_cell() {
        let (state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();

        runtime
            .eval_str("(neural-weights \"drums\" '((0 0.5 0) (0 0 0.25) (1 0 0)))")
            .unwrap();
        let networks = state.current_neural_networks();
        assert_eq!(
            networks[0].weights,
            vec![
                vec![0.0, 0.5, 0.0],
                vec![0.0, 0.0, 0.25],
                vec![1.0, 0.0, 0.0],
            ]
        );

        let updated = runtime
            .eval_str("(neural-weight \"drums\" :from 0 :to 2 :value 0.9)")
            .unwrap();
        assert!(matches!(updated, Some(Value::Map(_))));
        assert_eq!(state.current_neural_networks()[0].weights[0][2], 0.9);
    }

    #[test]
    fn neural_lisp_selects_and_clears_neuron_selection() {
        let (_state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();

        let selected = runtime
            .eval_str("(neural-select-neuron \"drums\" 2)")
            .unwrap()
            .expect("selection list");
        let Value::List(items) = selected else {
            panic!("expected selected neuron list");
        };
        assert_eq!(items.len(), 1);
        let Value::Map(selected) = &*items[0].borrow() else {
            panic!("expected selected neuron map");
        };
        assert_eq!(
            selected.get("pattern").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            selected
                .get("network-id")
                .map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            selected.get("neuron").map(|value| value.borrow().clone()),
            Some(Value::Number(2.0))
        );
        assert_eq!(
            runtime.eval_str("(neural-neuron-selected? 1 2)").unwrap(),
            Some(Value::Bool(true))
        );

        let cleared = runtime.eval_str("(neural-clear-selection)").unwrap();
        assert!(matches!(cleared, Some(Value::List(items)) if items.is_empty()));
        assert_eq!(
            runtime.eval_str("(neural-neuron-selected? 1 2)").unwrap(),
            Some(Value::Bool(false))
        );

        runtime
            .eval_str("(neural-select-neuron \"drums\" 1)")
            .unwrap();
        runtime.eval_str("(neural-delete \"drums\")").unwrap();
        assert_eq!(
            runtime.eval_str("(neural-selected-neurons)").unwrap(),
            Some(Value::List(vec![]))
        );
    }

    #[test]
    fn selected_neural_instrument_plock_helper_records_current_pattern_selection() {
        let (state, mut runtime) = neural_test_runtime(2);
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();

        let mut selection = BTreeSet::new();
        selection.insert(SelectedNeuralNeuron {
            pattern_idx: 0,
            network_id: 1,
            neuron_idx: 0,
        });

        let wrote =
            set_selected_neural_instrument_plocks(&state, &selection, 1, speed_param_idx, 1.25)
                .unwrap();
        assert!(wrote);
        assert_eq!(
            selected_neural_instrument_plock_value(&state, &selection, 1, speed_param_idx),
            Some(1.25)
        );
        assert_eq!(
            state.current_neural_networks()[0].neurons[0]
                .output_overrides
                .instrument[0]
                .target_track,
            1
        );
    }

    #[test]
    fn neural_plock_clear_helpers_remove_single_network_entry() {
        let (state, mut runtime) = neural_test_runtime(2);
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {speed_param_idx} 1.5)"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-plock-effect \"router\" 0 1 0 0 800.0)")
            .unwrap();

        assert!(
            clear_neural_instrument_plock_by_network_id(&state, 1, 0, 1, speed_param_idx).unwrap()
        );
        assert!(clear_neural_effect_plock_by_network_id(&state, 1, 0, 1, 0, 0).unwrap());
        assert!(
            !clear_neural_instrument_plock_by_network_id(&state, 1, 0, 1, speed_param_idx).unwrap()
        );

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert!(neuron.output_overrides.instrument.is_empty());
        assert!(neuron.output_overrides.effects.is_empty());
    }

    #[test]
    fn neural_lisp_plock_authoring_targets_tracks_and_devices() {
        let (state, mut runtime) = neural_test_runtime(2);
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let sampler_speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        let sampler_speed_node_param_idx =
            sampler_desc.params[sampler_speed_param_idx].node_param_idx;
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {sampler_speed_param_idx} 1.5)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {sampler_speed_param_idx} 2.0)"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-plock-effect \"router\" 0 1 0 0 800.0)")
            .unwrap();

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert_eq!(
            neuron.output_overrides.instrument,
            vec![crate::neural::ProjectParamOverride {
                target_track: 1,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: sampler_speed_node_param_idx,
                },
                param_index: sampler_speed_param_idx,
                value: 2.0,
            }]
        );
        assert_eq!(
            neuron.output_overrides.effects,
            vec![crate::neural::ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: EffectDescriptor::builtin_filter().params[0].node_param_idx,
                },
                param_index: 0,
                value: 800.0,
            }]
        );

        runtime
            .eval_str(&format!(
                "(neural-clear-instrument-plock \"router\" 0 1 {sampler_speed_param_idx})"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-clear-effect-plock \"router\" 0 1 0 0)")
            .unwrap();

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert!(neuron.output_overrides.instrument.is_empty());
        assert!(neuron.output_overrides.effects.is_empty());
    }

    #[test]
    fn neural_lisp_track_router_script_is_idempotent_and_routes_tracks() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(format!(
            "{}/scripts/neural-8x8-track-router.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read neural router script");

        let first = runtime.eval_str(&source).unwrap();
        let first_status = runtime.take_status_message();
        assert!(
            matches!(first, Some(Value::Map(_))),
            "expected first script eval to return map, got {first:?}; status {first_status:?}"
        );
        let second = runtime.eval_str(&source).unwrap();
        assert!(
            matches!(second, Some(Value::Map(_))),
            "expected second script eval to return map, got {second:?}"
        );

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        let network = &networks[0];
        assert_eq!(network.name, "8x8-track-router2");
        assert_eq!(network.num_neurons, 8);
        assert_eq!(network.reset_interval_bars, 4.0);
        assert_eq!(network.energy_decay, 0.994);
        assert_eq!(network.max_poly, 2);
        assert_eq!(
            network.max_poly_selection,
            NeuralMaxPolySelection::Deterministic
        );
        assert_eq!(
            network.weights,
            vec![
                vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ]
        );
        let routes = network
            .neurons
            .iter()
            .map(|neuron| neuron.route)
            .collect::<Vec<_>>();
        assert_eq!(
            routes,
            vec![
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(6),
                Some(7),
            ]
        );
        assert_eq!(
            network
                .neurons
                .iter()
                .map(|neuron| neuron.delay_steps)
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1, 1, 1, 1]
        );
        assert!(network
            .neurons
            .iter()
            .all(|neuron| neuron.quantize_timebase().is_none()));
        assert!(network.neurons.iter().all(|neuron| neuron.transpose == 0.0));
        assert!(network.neurons.iter().all(|neuron| neuron.threshold == 1.0));
        assert!(network
            .neurons
            .iter()
            .all(|neuron| neuron.dampening_amount == 0.0));
        assert!(network
            .neurons
            .iter()
            .all(|neuron| (neuron.dampening_recovery - 0.98).abs() < f32::EPSILON));
        assert!(!state.pattern.neural_reset_patterns[0].is_active(0));
    }

    #[test]
    fn neural_lisp_track_router_route_dropdown_supports_track_16() {
        let (state, mut runtime) = neural_test_runtime(16);
        let source = std::fs::read_to_string(format!(
            "{}/scripts/neural-8x8-track-router.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let options = runtime
            .eval_str("neural-8x8-track-router-route-options")
            .unwrap()
            .expect("route options");
        let Value::List(options) = options else {
            panic!("expected route options list, got {options:?}");
        };
        assert!(
            options.iter().any(
                |option| matches!(&*option.borrow(), Value::String(value) if value == "Track 16")
            ),
            "route dropdown should include Track 16"
        );
        assert_eq!(
            runtime
                .eval_str("(neural-8x8-track-router-route-index \"Track 16\")")
                .unwrap(),
            Some(Value::Number(15.0))
        );
        assert_eq!(
            runtime
                .eval_str("(neural-8x8-track-router-route-label 15)")
                .unwrap(),
            Some(Value::String("Track 16".to_string()))
        );

        runtime
            .eval_str(
                "(do
                  (set! neural-8x8-track-router-route-0 \"Track 16\")
                  (neural-8x8-track-router-apply-neuron-0))",
            )
            .unwrap();
        assert_eq!(
            state.current_neural_networks()[0].neurons[0].route,
            Some(15)
        );
    }

    #[test]
    fn neural_lisp_track_router_reuses_existing_named_network() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(format!(
            "{}/scripts/neural-8x8-track-router.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let initial = state.current_neural_networks();
        assert_eq!(initial.len(), 1);
        let id = initial[0].id;

        runtime
            .eval_str(&format!("(neural-weight {id} :from 0 :to 1 :value 0.25)"))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-set {id} :reset-bars 2 :energy-decay 0.5 :max-poly 4 :max-poly-selection :random)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-neuron {id} 1 :route 7 :threshold 1.5 :delay 4 :quantize :4 :transpose -7 :dampening 0.25 :recovery 0.75)"
            ))
            .unwrap();

        let second = runtime.eval_str(&source).unwrap();
        assert!(
            matches!(second, Some(Value::Map(_))),
            "expected router script to describe reused network, got {second:?}"
        );

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        let network = &networks[0];
        assert_eq!(network.id, id);
        assert_eq!(network.name, "8x8-track-router2");
        assert_eq!(network.reset_interval_bars, 2.0);
        assert_eq!(network.energy_decay, 0.5);
        assert_eq!(network.max_poly, 4);
        assert_eq!(network.max_poly_selection, NeuralMaxPolySelection::Random);
        assert_eq!(network.weights[0][1], 0.25);
        assert_eq!(network.neurons[1].route, Some(7));
        assert_eq!(network.neurons[1].threshold, 1.5);
        assert_eq!(network.neurons[1].delay_steps, 4);
        assert_eq!(
            network.neurons[1].quantize_timebase(),
            Some(crate::sequencer::Timebase::Quarter)
        );
        assert_eq!(network.neurons[1].transpose, -7.0);
        assert_eq!(network.neurons[1].dampening_amount, 0.25);
        assert_eq!(network.neurons[1].dampening_recovery, 0.75);
    }

    #[test]
    fn neural_lisp_track_router_reactive_refresh_loads_model_state() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(format!(
            "{}/scripts/neural-8x8-track-router.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let id = state.current_neural_networks()[0].id;
        let _ = runtime.take_pending_buffer_widget_trees();

        state
            .edit_current_neural_networks(|networks| {
                let network = networks
                    .iter_mut()
                    .find(|network| network.id == id)
                    .expect("router network");
                network.reset_interval_bars = 3.0;
                network.energy_decay = 0.5;
                network.max_poly = 5;
                network.max_poly_selection = NeuralMaxPolySelection::Random;
                network.weights[0][1] = 0.75;
                network.neurons[0].threshold = 1.75;
                network.neurons[1].route = Some(6);
                network.neurons[1].threshold = 2.5;
                network.neurons[1].delay_steps = 5;
                network.neurons[1].quantize = Some(crate::sequencer::Timebase::Eighth as u8);
                network.neurons[1].transpose = 12.0;
                network.neurons[1].dampening_amount = 0.33;
                network.neurons[1].dampening_recovery = 0.44;
                Ok(())
            })
            .unwrap();

        let epoch_before_refresh = state.transport.pattern_epoch.load(Ordering::Relaxed);
        let outcome = runtime.set_reactive(
            "SEQ",
            "neural-networks",
            Value::List(vec![Rc::new(RefCell::new(Value::Number(id as f64)))]),
        );
        assert!(
            outcome.effects_dirty,
            "router panel should subscribe to SEQ.neural-networks"
        );
        runtime.run_reactive_cycle();
        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before_refresh,
            "reactive panel refresh should not write back unchanged network data"
        );

        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-reset-bars")
                .unwrap(),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-energy-decay")
                .unwrap(),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-max-poly")
                .unwrap(),
            Some(Value::Number(5.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-max-poly-selection")
                .unwrap(),
            Some(Value::String("random".to_string()))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-threshold")
                .unwrap(),
            Some(Value::Number(1.75))
        );
        assert_eq!(
            runtime.eval_str("neural-8x8-track-router-route-1").unwrap(),
            Some(Value::String("Track 7".to_string()))
        );
        assert_eq!(
            runtime.eval_str("neural-8x8-track-router-delay-1").unwrap(),
            Some(Value::Number(5.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-quantize-1")
                .unwrap(),
            Some(Value::String("8".to_string()))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-transpose-1")
                .unwrap(),
            Some(Value::Number(12.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-dampening-1")
                .unwrap(),
            Some(Value::Number(0.33_f32 as f64))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-recovery-1")
                .unwrap(),
            Some(Value::Number(0.44_f32 as f64))
        );
        assert!(
            !runtime.take_pending_buffer_widget_trees().is_empty(),
            "reactive refresh should rebuild the matrix buffer"
        );
    }

    #[test]
    fn neural_lisp_track_router_controls_align_with_matrix_rows() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_width(node: &eseqlisp::layout::LayoutNode, expected: f32, label: &str) {
            assert!(
                (node.rect.width - expected).abs() <= 0.05,
                "{label} should measure to width {expected}, got {:?}",
                node.rect
            );
        }

        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(format!(
            "{}/scripts/neural-8x8-track-router.lisp",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("router script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 18.0)))
            .expect("router widget tree should lay out");

        for (key, text, width) in [
            ("neural-router-column-label-route", "route", 7.68),
            ("neural-router-column-label-delay", "delay", 5.04),
            ("neural-router-column-label-quantize", "quant", 5.76),
            ("neural-router-column-label-transpose", "transp", 5.04),
            ("neural-router-column-label-dampening", "damp", 5.04),
            ("neural-router-column-label-recovery", "recov", 5.04),
        ] {
            let label = find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(label.widget_type, "label", "{key}");
            assert_eq!(
                label.props.get("text"),
                Some(&Value::String(text.to_string())),
                "{key}"
            );
            assert_measured(label);
            assert_width(label, width, key);
        }

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected trigger, energy, weight, and dampening matrix widgets"
        );
        matrices.sort_by(|left, right| left.rect.col.total_cmp(&right.rect.col));
        for matrix in &matrices {
            assert_measured(matrix);
        }
        let matrix = matrices[2];
        for (idx, visualization_matrix) in matrices.iter().enumerate() {
            assert!(
                (matrix.rect.row - visualization_matrix.rect.row).abs() <= 0.05
                    && (matrix.rect.height - visualization_matrix.rect.height).abs() <= 0.05,
                "visualization matrix {idx} should align with weight matrix; weight={:?} visualization={:?}",
                matrix.rect,
                visualization_matrix.rect
            );
        }

        let label_3 =
            find_by_stable_key(&layout, "neural-router-row-label-3").expect("row 3 label");
        let label_click = label_3
            .props
            .get("on-click")
            .cloned()
            .expect("row label on-click");
        runtime
            .invoke(label_click, vec![Value::Bool(true)])
            .expect("invoke row label click");
        assert_eq!(
            runtime
                .eval_str("(neural-neuron-selected? neural-8x8-track-router-id 2)")
                .unwrap(),
            Some(Value::Bool(true))
        );
        let row_3 = find_by_stable_key(&layout, "neural-router-row-3").expect("selected row 3");
        let mut row_3_dropdowns = Vec::new();
        collect_widgets(row_3, "dropdown", &mut row_3_dropdowns);
        assert_eq!(
            row_3_dropdowns.len(),
            2,
            "row 3 should contain route and quantize dropdowns"
        );
        row_3_dropdowns.sort_by(|left, right| left.rect.col.total_cmp(&right.rect.col));
        assert_width(row_3_dropdowns[0], 7.68, "row route dropdown");
        assert_width(row_3_dropdowns[1], 5.76, "row quantize dropdown");

        let mut row_3_pickers = Vec::new();
        collect_widgets(row_3, "number-picker", &mut row_3_pickers);
        assert_eq!(
            row_3_pickers.len(),
            4,
            "row 3 should contain delay, transpose, dampening, and recovery pickers"
        );
        for picker in row_3_pickers {
            assert_width(picker, 5.04, "row number picker");
        }

        let expected_selected_field = format!(
            "neural-neuron-selected-0-{}-2",
            state.current_neural_networks()[0].id
        );
        assert!(
            matches!(
                row_3.props.get("selected"),
                Some(Value::ReactiveRef { namespace, field, .. })
                    if namespace == "SEQ" && field == &expected_selected_field
            ),
            "row 3 should bind selected state to its targeted neural selection field"
        );
        assert_eq!(
            row_3.props.get("selected-background-color"),
            Some(&Value::Keyword("fx-panel-header-selected-bg".to_string()))
        );

        let clear_callback = layout
            .props
            .get("on-click")
            .cloned()
            .expect("outer panel click clears selection");
        runtime
            .invoke(clear_callback, vec![Value::Bool(true)])
            .expect("invoke outer panel click");
        assert_eq!(
            runtime.eval_str("(neural-selected-neurons)").unwrap(),
            Some(Value::List(vec![]))
        );

        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            17,
            "expected one global max-poly dropdown plus route and quantize dropdowns"
        );
        for dropdown in &dropdowns {
            assert_measured(dropdown);
        }

        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            36,
            "expected four global pickers and four pickers per neuron"
        );
        for picker in &pickers {
            assert_measured(picker);
        }

        let mut row_pickers = pickers
            .into_iter()
            .filter(|picker| {
                let center = picker.rect.row + picker.rect.height * 0.5;
                center >= matrix.rect.row && center <= matrix.rect.row + matrix.rect.height
            })
            .collect::<Vec<_>>();
        assert_eq!(
            row_pickers.len(),
            32,
            "expected four row-aligned pickers per neuron"
        );
        row_pickers.sort_by(|left, right| {
            left.rect
                .row
                .total_cmp(&right.rect.row)
                .then(left.rect.col.total_cmp(&right.rect.col))
        });

        let matrix_row_height = matrix.rect.height / 8.0;
        for (idx, picker) in row_pickers.iter().enumerate() {
            let row_idx = idx / 4;
            let expected_center = matrix.rect.row + matrix_row_height * (row_idx as f32 + 0.5);
            let actual_center = picker.rect.row + picker.rect.height * 0.5;
            assert!(
                (actual_center - expected_center).abs() <= 0.05,
                "row picker {idx} center {actual_center} should align with matrix row center {expected_center}; picker={:?} matrix={:?}",
                picker.rect,
                matrix.rect
            );
        }
    }

    #[test]
    fn neural_lisp_reset_step_sets_dedicated_flag() {
        let (state, mut runtime) = neural_test_runtime(1);

        let enabled = runtime
            .eval_str("(neural-reset-step :track 0 :step 4 true)")
            .unwrap();
        assert_eq!(enabled, Some(Value::Bool(true)));
        assert!(state.pattern.neural_reset_patterns[0].is_active(4));

        let disabled = runtime.eval_str("(neural-reset-step 0 4 false)").unwrap();
        assert_eq!(disabled, Some(Value::Bool(false)));
        assert!(!state.pattern.neural_reset_patterns[0].is_active(4));
    }

    #[test]
    fn neural_lisp_rejects_bad_matrix_shape() {
        let (state, mut runtime) = neural_test_runtime(1);

        let result = runtime
            .eval_str("(neural-create :name \"bad\" :neurons 2 :weights '((0 1)))")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(false)));
        assert!(state.current_neural_networks().is_empty());
    }

    #[test]
    fn neural_lisp_rejects_ambiguous_name_lookup() {
        let (state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"same\" :neurons 1)")
            .unwrap();
        runtime
            .eval_str("(neural-create :name \"same\" :neurons 1)")
            .unwrap();

        let result = runtime.eval_str("(neural-describe \"same\")").unwrap();

        assert_eq!(result, Some(Value::Bool(false)));
        assert_eq!(state.current_neural_networks().len(), 2);
    }

    #[test]
    fn seq_step_returns_map_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step 0)").unwrap();
        assert!(matches!(result, Some(Value::Map(_))));
    }

    #[test]
    fn seq_track_steps_returns_list_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-track-steps)").unwrap();
        assert!(matches!(result, Some(Value::List(_))));
    }

    #[test]
    fn seq_set_current_track_updates_context_for_following_calls() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(2),
                fallback_instrument_descriptors(2),
            ),
        );

        let result = runtime.eval_str("(seq-set-current-track 1)").unwrap();
        assert_eq!(result, Some(Value::Number(1.0)));

        let result = runtime.eval_str("(seq-current-track)").unwrap();
        assert_eq!(result, Some(Value::Number(1.0)));

        let result = runtime.eval_str("(seq-toggle-step 0)").unwrap();
        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[1].is_active(0));
        assert!(!state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn seq_step_on_activates_step_without_toggle_semantics() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step-on 2)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[0].is_active(2));
    }

    #[test]
    fn seq_step_off_clears_payload_and_deactivates_step() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.8);
        state.pattern.effect_chains[0][0].set_plock(2, 0, 0.25);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step-off 2)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.effect_chains[0][0].plocks.get(2, 0), None);
    }

    #[test]
    fn seq_rotate_track_rotates_full_pattern() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 7.0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );
        let result = runtime.eval_str("(seq-rotate-track 1)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 7.0);
    }

    #[test]
    fn seq_plock_step_sets_step_param() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime
            .eval_str("(seq-plock-step 1 :velocity 0.7)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Velocity), 0.7);
    }

    #[test]
    fn seq_plock_timebase_sets_timebase_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-plock-timebase 2 :8t)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.timebase_plocks[0].get(2),
            Some(crate::sequencer::Timebase::EighthTriplet)
        );
    }

    #[test]
    fn seq_plock_effect_normalizes_slot_param_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        bind_filter_slot(&state);
        let mut runtime = Runtime::new();
        let effect_descriptors = descriptors_with_filter(1);
        let expected = effect_descriptors[0][0].params[2].denormalize(0.5);
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(effect_descriptors, fallback_instrument_descriptors(1)),
        );

        let result = runtime
            .eval_str("(seq-plock-effect 0 FILTER.cutoff 0.5)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(0, 2),
            Some(expected)
        );
    }

    #[test]
    fn seq_plock_effect_raw_preserves_stored_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        bind_filter_slot(&state);
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime
            .eval_str("(seq-plock-effect-raw 0 0 2 440.0)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(0, 2),
            Some(440.0)
        );
    }

    #[test]
    fn seq_effect_param_name_returns_effect_param_name() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-effect-param-name 0 2)").unwrap();

        assert_eq!(result, Some(Value::String("cutoff".to_string())));
    }

    #[test]
    fn seq_effect_param_names_returns_effect_param_name_list() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-effect-param-names 0)").unwrap();

        match result {
            Some(Value::List(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::String(name) => name.clone(),
                        other => panic!("expected string, got {other:?}"),
                    })
                    .collect();
                assert!(names.starts_with(&[
                    "enabled".to_string(),
                    "mode".to_string(),
                    "cutoff".to_string(),
                    "resonance".to_string(),
                ]));
                assert!(names.contains(&"drive".to_string()));
                assert!(names.contains(&"lfo amt".to_string()));
                assert!(names.contains(&"env amt".to_string()));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn seq_effect_globals_expose_slot_and_param_refs() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("FILTER.cutoff").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0, 2.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn seq_instrument_globals_expose_param_refs() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let custom_desc = EffectDescriptor::from_lisp_manifest(
            "MINIMOOG",
            &[DGenParam {
                name: "cutoff".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            0,
            0,
        );
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![custom_desc]),
        );

        let result = runtime.eval_str("MINIMOOG.cutoff").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn scratch_runtime_with_fallbacks_uses_state_published_descriptors() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let custom_desc = EffectDescriptor::from_lisp_manifest(
            "MODUM_DELAY",
            &[DGenParam {
                name: "max1".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.0,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            2,
            2,
        );
        let mut effect_descriptors = fallback_effect_descriptors(1);
        effect_descriptors[0][0] = custom_desc;
        state.set_scratch_runtime_descriptors(
            effect_descriptors,
            fallback_instrument_descriptors(1),
        );

        let mut runtime = scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
        let result = runtime.eval("MODUM_DELAY.max1").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0, 0.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn seq_plock_instrument_normalizes_slot_param_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::from_lisp_manifest(
            "MINIMOOG",
            &[DGenParam {
                name: "cutoff".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            0,
            0,
        );
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);
        let expected = instrument_desc.params[0].denormalize(0.25);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime
            .eval_str("(seq-plock-instrument 0 MINIMOOG.cutoff 0.25)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(0, 0),
            Some(expected)
        );
    }

    #[test]
    fn seq_instrument_param_name_returns_instrument_param_name() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::builtin_delay();
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime.eval_str("(seq-instrument-param-name 2)").unwrap();

        assert_eq!(result, Some(Value::String("time".to_string())));
    }

    #[test]
    fn seq_instrument_param_names_returns_instrument_param_name_list() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::builtin_delay();
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime.eval_str("(seq-instrument-param-names)").unwrap();

        match result {
            Some(Value::List(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::String(name) => name.clone(),
                        other => panic!("expected string, got {other:?}"),
                    })
                    .collect();
                assert_eq!(
                    names,
                    vec!["wet", "synced", "time", "feedback", "dampening", "width"]
                );
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn seq_step_shows_value_through_editor_eval_binding() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let init_src = read_eseqlisp_init_source();
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init_src),
                ..EditorConfig::default()
            },
        );
        editor.open_scratch_buffer_with_mode("*scratch*", "(seq-step 0)", BufferMode::ESeqLisp);
        editor.active_buffer_mut().cursor = (0, "(seq-step 0)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        let minibuffer = editor.minibuffer.unwrap_or_default();
        assert!(minibuffer.contains("step"), "minibuffer was: {minibuffer}");
    }

    #[test]
    fn scratch_control_runtime_can_invoke_exported_closure() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );

        let callback = runtime
            .eval("(lambda () (seq-toggle-step 0))")
            .unwrap()
            .unwrap();
        runtime.set_global_value("__hook_test", callback);
        let result = runtime.eval("(__hook_test)").unwrap().unwrap();

        assert_eq!(result, Value::Bool(true));
        assert!(state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn scratch_control_runtime_runs_source_hooks_with_dynamic_track_context() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.set_position(1, 0);
        let result = runtime.eval("(seq-toggle-step 0)").unwrap().unwrap();

        assert_eq!(result, Value::Bool(true));
        assert!(state.pattern.patterns[1].is_active(0));
        assert!(!state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn scratch_control_runtime_registers_and_invokes_accumulator_callbacks() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "test-acc"
                  (do
                     (acc-add-step-param :transpose acc-value)
                     (acc-scale-step-param :velocity 0.5)
                     (acc-set-step-param :pan 0.25)
                     (acc-add-effect-param FILTER.cutoff 0.25)
                     (acc-add-instrument-param 0 0.25)))
                "#,
            )
            .unwrap();

        assert_eq!(runtime.accumulator_names(), vec!["test-acc".to_string()]);

        let effect_desc = EffectDescriptor::builtin_filter();
        let effect_initial = effect_desc.params[2].denormalize(0.5);
        let effect_expected = effect_desc.params[2].denormalize(0.75);
        let instrument_desc = fallback_instrument_descriptors(1)[0].clone();
        let instrument_initial = instrument_desc.params[0].denormalize(0.5);
        let instrument_expected = instrument_desc.params[0].denormalize(0.75);

        let output = runtime
            .invoke_accumulator(
                0,
                3,
                3.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 2.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![1.0, 1.0, 1.0],
                2.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 2,
                    value: effect_initial,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: instrument_initial,
                }],
            )
            .unwrap();

        assert_eq!(output.resolved.transpose, 5.0);
        assert_eq!(output.resolved.velocity, 0.5);
        assert_eq!(output.resolved.pan, 0.25);
        assert!(output.effect_params.iter().any(|param| {
            param.logical_id == 42
                && param.idx == 2
                && (param.value - effect_expected).abs() < 0.001
        }));
        assert!(output.instrument_params.iter().any(|param| param.target
            == ScheduledInstrumentParamTarget::Synth
            && param.idx == 0
            && (param.value - instrument_expected).abs() < 0.001));
    }

    #[test]
    fn scratch_control_runtime_clamps_normalized_accumulator_param_adds() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "clip-acc"
                  (do
                     (acc-add-effect-param FILTER.cutoff 0.75)
                     (acc-add-instrument-param 0 0.75)))
                "#,
            )
            .unwrap();

        let effect_desc = EffectDescriptor::builtin_filter();
        let effect_initial = effect_desc.params[2].denormalize(0.5);
        let effect_expected = effect_desc.params[2].denormalize(1.0);
        let instrument_desc = fallback_instrument_descriptors(1)[0].clone();
        let instrument_initial = instrument_desc.params[0].denormalize(0.5);
        let instrument_expected = instrument_desc.params[0].denormalize(1.0);

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                1.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(&effect_desc, 42)],
                EffectSlotSnapshot::new_default(&instrument_desc, 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 2,
                    value: effect_initial,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: instrument_initial,
                }],
            )
            .unwrap();

        assert!(output.effect_params.iter().any(|param| {
            param.logical_id == 42
                && param.idx == 2
                && (param.value - effect_expected).abs() < 0.001
        }));
        assert!(output.instrument_params.iter().any(|param| {
            param.target == ScheduledInstrumentParamTarget::Synth
                && param.idx == 0
                && (param.value - instrument_expected).abs() < 0.001
        }));
    }

    #[test]
    fn scratch_control_runtime_accumulator_can_emit_arp_events() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp"
                  (do
                     (acc-suppress)
                     (acc-emit 0 :note 0 :vel 0.9)
                     (acc-emit 1 :note 4 :vel 0.8)
                     (acc-emit :8t 1 :note 7 :track 0)))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![1.0, 1.0, 1.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(output.suppressed);
        assert_eq!(output.emitted.len(), 3);
        assert_eq!(output.emitted[0].offset_beats, 0.0);
        assert_eq!(output.emitted[0].resolved.transpose, 0.0);
        assert_eq!(output.emitted[0].resolved.velocity, 0.9);
        assert!(output.emitted[0].chord.is_empty());
        assert_eq!(output.emitted[1].offset_beats, 0.25);
        assert_eq!(output.emitted[1].resolved.transpose, 4.0);
        assert!((output.emitted[2].offset_beats - (1.0 / 3.0)).abs() < 0.0001);
        assert_eq!(output.emitted[2].track, Some(0));
    }

    #[test]
    fn scratch_control_runtime_def_accumulator_wrong_arity_errors() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        assert!(runtime.eval(r#"(def-accumulator "missing-body")"#).is_err());
        assert!(runtime
            .eval(
                r#"
                (def-accumulator "multi-body"
                  (acc-suppress)
                  (acc-emit 0))
                "#
            )
            .is_err());
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_follow_chord_durations() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 6.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![6.0, 6.0, 6.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert_eq!(notes, vec![0.0, 4.0, 7.0, 0.0, 4.0, 7.0]);
        assert_eq!(output.emitted.len(), 6);
        assert_eq!(output.emitted[5].offset_beats, 1.25);
        assert_eq!(output.emitted[5].resolved.duration, 1.0);
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_fall_back_to_step_duration() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 6.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![0.0, 0.0, 0.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert_eq!(output.emitted.len(), 6);
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_use_joined_note_pool() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![
                    AccumulatorNoteSpan {
                        transpose: 0.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 4.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 7.0,
                        start_beats: 1.0,
                        end_beats: 2.0,
                    },
                ]),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert_eq!(notes, vec![0.0, 4.0, 0.0, 4.0, 4.0, 7.0, 0.0, 4.0]);
        assert_eq!(output.emitted.len(), 8);
    }

    #[test]
    fn scratch_control_runtime_arp_source_assigns_track() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-16"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))

                (seq-use-accumulator 0 "arp-16")
                "#,
            )
            .unwrap();

        let params = &state.pattern.track_params[0];
        assert_eq!(params.script_accumulator_name(), Some("arp-16".to_string()));
        assert_eq!(
            params.get_accumulator_idx(),
            crate::accumulator::ACCUMULATOR_REGISTRY.len()
        );
    }

    #[test]
    fn scratch_control_runtime_midi_fx_source_assigns_track_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "arp-16"
                  (do
                    (fx-suppress)
                    (for-each |i|
                      (fx-arp-emit :16 i :vel 0.8)
                      (range 0 (fx-arp-count :16)))))

                (seq-use-midi-fx 0 "arp-16")
                "#,
            )
            .unwrap();

        let params = &state.pattern.track_params[0];
        assert_eq!(runtime.midi_fx_names(), vec!["arp-16".to_string()]);
        assert_eq!(params.midi_fx_chain(), vec!["arp-16".to_string()]);
        assert_eq!(
            params.get_midi_fx_position(),
            crate::sequencer::MidiFxPosition::PostAccumulator
        );
    }

    #[test]
    fn folder_midi_fx_registers_params_and_syncs_track_slot() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(&super::midi_fx_library_source_with_user_source(
                r#"
                (seq-use-midi-fx 0 "arp")
                (seq-set-midi-fx-param 0 "rate" 3)
                (seq-plock-midi-fx 0 0 "rate" 9)
                "#,
            ))
            .unwrap();

        let params = &state.pattern.track_params[0];
        let slot = &state.pattern.midi_fx_slots[0][0];
        assert!(runtime.midi_fx_names().iter().any(|name| name == "arp"));
        assert_eq!(params.midi_fx_chain(), vec!["arp".to_string()]);
        assert_eq!(slot.num_params.load(Ordering::Relaxed), 6);
        assert_eq!(slot.defaults.get(0), 3.0);
        assert_eq!(slot.plocks.get(0, 0), Some(9.0));
    }

    #[test]
    fn scratch_control_runtime_midi_fx_can_emit_joined_arp_events() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "arp-held"
                  (do
                    (fx-suppress)
                    (for-each |i|
                      (fx-arp-emit :16 i :vel 0.8)
                      (range 0 (fx-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![
                    AccumulatorNoteSpan {
                        transpose: 0.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 4.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 7.0,
                        start_beats: 1.0,
                        end_beats: 2.0,
                    },
                ]),
                EffectSlotSnapshot::new_empty(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert!(output.suppressed);
        assert_eq!(notes, vec![0.0, 4.0, 0.0, 4.0, 4.0, 7.0, 0.0, 4.0]);
        assert_eq!(output.emitted.len(), 8);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_arp_octave_expands_note_pool() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let arp_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "arp")
            .expect("arp descriptor");
        let mut slot = EffectSlotSnapshot::new_default(&arp_desc, 0);
        slot.defaults[2] = 2.0;

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![8.0, 8.0, 8.0],
                0.0,
                None,
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert!(output.suppressed);
        assert_eq!(notes, vec![0.0, 4.0, 7.0, 12.0, 16.0, 19.0, 0.0, 4.0]);
    }

    #[test]
    fn folder_midi_fx_trigger_to_track_emits_to_selected_target_and_ignores_self() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let descriptors = runtime.midi_fx_descriptors();
        let trigger_idx = descriptors
            .iter()
            .position(|desc| desc.name == "trigger-to-track")
            .expect("trigger-to-track MIDI FX is registered");
        let trigger_desc = descriptors
            .get(trigger_idx)
            .expect("trigger-to-track descriptor");
        assert_eq!(trigger_desc.params.len(), 2);
        assert_eq!(trigger_desc.params[0].name, "track");
        assert_eq!(trigger_desc.params[1].name, "enabled");

        let mut slot = EffectSlotSnapshot::new_default(trigger_desc, 0);
        slot.defaults[0] = 2.0;
        let output = runtime
            .invoke_midi_fx(
                trigger_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0],
                vec![1.0],
                0.0,
                None,
                slot.clone(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        assert!(!output.suppressed);
        assert_eq!(output.emitted.len(), 1);
        assert_eq!(output.emitted[0].track, Some(1));
        assert_eq!(output.emitted[0].offset_beats, 0.0);
        assert_eq!(output.emitted[0].resolved.transpose, 5.0);

        slot.defaults[0] = 1.0;
        let self_target_output = runtime
            .invoke_midi_fx(
                trigger_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0],
                vec![1.0],
                0.0,
                None,
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        assert!(!self_target_output.suppressed);
        assert!(self_target_output.emitted.is_empty());
    }

    #[test]
    fn scratch_control_runtime_midi_fx_uses_beat_timing_helpers() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "timing"
                  (do
                    (fx-suppress)
                    (fx-emit :beats (fx-time :8t 1))
                    (fx-emit :beats (fx-source-time 2))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                None,
                EffectSlotSnapshot::new_empty(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(output.suppressed);
        assert!((output.emitted[0].offset_beats - (1.0 / 3.0)).abs() < 0.0001);
        assert_eq!(output.emitted[1].offset_beats, 0.5);
    }

    #[test]
    fn registered_sequencer_tick_emits_event() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(__register-sequencer "chord"
                     :resolution :1
                     :tick (lambda () (seq-emit :track 0 :at :now :vel 0.9 :chord (list 0 4 7))))"#,
            )
            .expect("register sequencer");

        let defs = runtime.sequencer_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "chord");
        assert!((defs[0].resolution_beats - 4.0).abs() < 1e-9); // :1 = whole = 4 beats

        let result = runtime
            .invoke_sequencer_tick(
                0,
                crate::generator::GeneratorTickInput {
                    id: defs[0].id,
                    generator_index: 0,
                    tick_index: 0,
                    beat: 0.0,
                    resolution_beats: defs[0].resolution_beats,
                    samples_per_quarter: 48_000.0,
                    random_state: 1,
                    state: Default::default(),
                },
            )
            .expect("tick");

        assert_eq!(result.emitted.len(), 1);
        let event = &result.emitted[0];
        assert_eq!(event.track, Some(0));
        assert_eq!(event.offset_beats, 0.0);
        assert!((event.resolved.velocity - 0.9).abs() < 1e-6);
        assert_eq!(event.chord, vec![0.0, 4.0, 7.0]);
    }

    #[test]
    fn def_sequencer_drives_generator_runtime_end_to_end() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(def-sequencer "chord"
                     :resolution :1
                     :tick (seq-emit :track 0 :at :now :vel 0.8 :chord (list 0 4 7)))"#,
            )
            .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&runtime.sequencer_defs(), 0.0);
        assert_eq!(generators.len(), 1);

        // Drive exactly as the scheduler does: tick the generator runtime, routing
        // each boundary through the scheduler-side VM's :tick closure.
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                runtime
                    .invoke_sequencer_tick(input.generator_index, input)
                    .expect("tick")
            },
            &mut out,
        );

        // :1 = whole note = 4 beats; one boundary at beat 4.0 within (0, 4].
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.track, Some(0));
        assert_eq!(out[0].event.chord, vec![0.0, 4.0, 7.0]);
        assert_eq!(out[0].sample_time, 192_000); // 4 beats * 48000 spq
    }

    #[test]
    fn def_sequencer_state_cells_persist_across_ticks() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(def-sequencer "counter"
                     :resolution :4
                     :tick (do
                       (state-set! "n" (+ 1 (state-get "n" 0)))
                       (seq-emit :track 0 :at :now :note (state-get "n"))))"#,
            )
            .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&runtime.sequencer_defs(), 0.0);

        let mut out = Vec::new();
        generators.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                runtime
                    .invoke_sequencer_tick(input.generator_index, input)
                    .expect("tick")
            },
            &mut out,
        );

        // :4 = quarter = 1 beat; boundaries at 1,2,3,4 -> the counter persists across
        // ticks, so transpose climbs 1,2,3,4.
        let transposes: Vec<f32> = out.iter().map(|e| e.event.resolved.transpose).collect();
        assert_eq!(transposes, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn seq_emit_quantize_snaps_offset_to_grid() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(__register-sequencer "q"
                     :resolution :16
                     :tick (lambda () (seq-emit :track 0 :at :now :quantize :4)))"#,
            )
            .expect("register sequencer");

        // Boundary at beat 0.30 with :4 (quarter, 1 beat) quantize -> snap up to 1.0,
        // so offset = 1.0 - 0.30 = 0.70 beats.
        let result = runtime
            .invoke_sequencer_tick(
                0,
                crate::generator::GeneratorTickInput {
                    id: 0,
                    generator_index: 0,
                    tick_index: 0,
                    beat: 0.30,
                    resolution_beats: 0.25,
                    samples_per_quarter: 48_000.0,
                    random_state: 1,
                    state: Default::default(),
                },
            )
            .expect("tick");
        assert_eq!(result.emitted.len(), 1);
        assert!((result.emitted[0].offset_beats - 0.70).abs() < 1e-5);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_arp_phase_rotates_live_notes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();

        let invoke = |runtime: &mut ScratchControlRuntime, arp_phase_beats| {
            runtime
                .invoke_midi_fx_with_arp_phase_beats(
                    0,
                    0,
                    0,
                    0.0,
                    ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    vec![0.0, 4.0, 7.0],
                    vec![1.0, 1.0, 1.0],
                    0.0,
                    Some(vec![
                        AccumulatorNoteSpan {
                            transpose: 0.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                        AccumulatorNoteSpan {
                            transpose: 4.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                        AccumulatorNoteSpan {
                            transpose: 7.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                    ]),
                    EffectSlotSnapshot::new_empty(),
                    arp_phase_beats,
                    0.25,
                    16,
                    vec![EffectSlotSnapshot::new_default(
                        &EffectDescriptor::builtin_filter(),
                        42,
                    )],
                    EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap()
        };

        assert_eq!(invoke(&mut runtime, 0.0).emitted[0].resolved.transpose, 0.0);
        assert_eq!(
            invoke(&mut runtime, 0.25).emitted[0].resolved.transpose,
            4.0
        );
        assert_eq!(invoke(&mut runtime, 0.5).emitted[0].resolved.transpose, 7.0);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_state_persists_per_track() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "every-two-octave"
                  (do
                    (if (= (fx-state-get :count 0) 1)
                      (do
                        (fx-state-set :count 0)
                        (fx-emit 0 :transpose (+ 12 (fx-note 0))))
                      (fx-state-set :count 1))))
                "#,
            )
            .unwrap();

        let invoke = |runtime: &mut ScratchControlRuntime, track| {
            runtime
                .invoke_midi_fx(
                    0,
                    track,
                    0,
                    0.0,
                    ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    vec![0.0],
                    vec![1.0],
                    0.0,
                    None,
                    EffectSlotSnapshot::new_empty(),
                    0.25,
                    16,
                    vec![EffectSlotSnapshot::new_default(
                        &EffectDescriptor::builtin_filter(),
                        42,
                    )],
                    EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[track], 7),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap()
        };

        assert_eq!(invoke(&mut runtime, 0).emitted.len(), 0);
        assert_eq!(invoke(&mut runtime, 0).emitted[0].resolved.transpose, 12.0);
        assert_eq!(invoke(&mut runtime, 1).emitted.len(), 0);
        assert_eq!(invoke(&mut runtime, 1).emitted[0].resolved.transpose, 12.0);
    }

    #[test]
    fn scratch_control_runtime_can_register_closure_accumulator_directly() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        let result = runtime
            .eval(
                r#"
                (__register-accumulator "closure-acc"
                  (lambda (step value)
                    (do
                      (acc-add-step-param :transpose value)
                      (acc-scale-step-param :velocity 0.5)
                      (acc-set-step-param :pan 0.25)
                      (acc-set-effect-param 0 1 value)
                      (acc-set-instrument-param 0 0.75))))
                "#,
            )
            .unwrap();
        let status = runtime.take_status_message();

        assert_eq!(result, Some(Value::Bool(true)), "status: {status:?}");
        assert_eq!(
            runtime.accumulator_names(),
            vec!["closure-acc".to_string()],
            "status: {status:?}"
        );

        runtime
            .invoke_accumulator(
                0,
                3,
                3.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 2.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                2.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 1,
                    value: 0.0,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: 0.1,
                }],
            )
            .unwrap();
    }

    #[test]
    fn scratch_runtime_editor_loads_init_bindings_for_eval() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        )
        .into_parts()
        .0;
        let init_src = read_eseqlisp_init_source();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init_src),
                ..EditorConfig::default()
            },
        );
        editor.open_scratch_buffer_with_mode("*scratch*", "(+ 1 1)", BufferMode::ESeqLisp);
        editor.active_buffer_mut().cursor = (0, "(+ 1 1)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        assert_eq!(editor.minibuffer.unwrap_or_default(), "2");
    }

    #[test]
    fn parse_manifest_reads_wavetable_tensor_metadata() {
        let manifest = parse_manifest(
            r#"{
              "version": 1,
              "dylib": "test.dylib",
              "totalMemorySlots": 16,
              "params": [],
              "inputs": [],
              "outputs": [{"channel": 0, "name": "audio"}],
              "modulators": [],
              "modDestinations": [],
              "tensors": [
                {
                  "name": "waves",
                  "cellOffset": 4,
                  "shape": [2, 4],
                  "kind": "wavetable",
                  "mutable": false,
                  "sourceFile": "waves/tiny.json"
                }
              ],
              "tensorInitData": [
                {"offset": 4, "data": [0.0, 0.25, 0.5, 0.75, 1.0, 0.5, 0.0, -0.5]}
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].name, "waves");
        assert_eq!(manifest.tensors[0].cell_offset, 4);
        assert_eq!(manifest.tensors[0].shape, vec![2, 4]);
        assert_eq!(manifest.tensors[0].kind, "wavetable");
        assert!(!manifest.tensors[0].mutable);
        assert_eq!(
            manifest.tensors[0].source_file.as_deref(),
            Some("waves/tiny.json")
        );
        assert_eq!(manifest.tensor_init_data[0].offset, 4);
        assert_eq!(manifest.tensor_init_data[0].data.len(), 8);
    }

    #[test]
    fn compile_instrument_passes_asset_base_for_wavetable_files() {
        let root = std::env::temp_dir().join(format!(
            "sequencer-wavetable-asset-test-{}",
            std::process::id()
        ));
        let waves = root.join("waves");
        std::fs::create_dir_all(&waves).unwrap();
        std::fs::write(
            waves.join("tiny.json"),
            r#"{"shape":[4,2],"data":[0.0,1.0,0.25,0.5,0.5,0.0,0.75,-0.5]}"#,
        )
        .unwrap();

        let source = r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def waves (wavetable @shape [4 2] @file "waves/tiny.json"))
            (out (* (peek waves 1 0) gate velocity) 1 @name audio)
        "#;

        let json = compile_instrument_with_asset_base(source, 44_100, Some(&root)).unwrap();
        let manifest = parse_manifest(&json).unwrap();
        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].name, "waves");
        assert_eq!(manifest.tensors[0].shape, vec![4, 2]);
        assert_eq!(manifest.tensor_init_data[0].data.len(), 8);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compile_instrument_manifest_includes_modulation_outputs_from_out_forms() {
        let source = r#"
            (out (phasor 0.25) 2 @name macro-a @modulator 1)
            (out (phasor 50.0) 1 @name audio)
        "#;

        let json = compile_instrument(source, 44_100).expect("instrument compiles");
        let manifest = parse_manifest(&json).expect("manifest parses");

        assert_eq!(manifest.mod_outputs.len(), 1);
        let output = &manifest.mod_outputs[0];
        assert_eq!(output.slot, 1);
        assert_eq!(output.channel, 1);
        assert_eq!(output.name, "macro-a");
        assert_eq!(output.range, "unipolar");
    }

    #[test]
    fn patcher_writeback_for_real_instrument_compiles() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("instruments/bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted = eseqlisp::widget_render::patcher::emit_patch_writeback_source(
            &source,
            eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
        )
        .expect("patcher writeback should emit source");

        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-emitted instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn patcher_insert_unity_gain_before_real_instrument_output_compiles() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("instruments/bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_inserted_node_before_first_output(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "* 1",
            )
            .expect("patcher writeback should insert unity gain node");

        assert!(
            emitted.contains("(* "),
            "emitted source should contain an inserted multiply node:\n{emitted}"
        );
        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-edited instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn patcher_edit_piano_to_test_svf_literal_compiles() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("instruments/piano-to-test/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read piano-to-test dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_first_node_text_edit(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "svf",
                "svf knock_cutoff 1.40 1",
            )
            .expect("patcher writeback should edit svf node text");

        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| {
                panic!(
                    "patcher-edited piano-to-test svf source should compile:\n{error}\n{emitted}"
                )
            },
        );
    }

    #[test]
    fn patcher_insert_created_phasor_multiply_before_real_instrument_output_compiles() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("instruments/bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_created_phasor_multiply_before_first_output(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "5",
            )
            .expect("patcher writeback should insert created phasor multiply chain");

        assert!(
            emitted.contains("(phasor 5.0)") && emitted.contains("(* "),
            "emitted source should contain the inserted phasor multiply chain:\n{emitted}"
        );
        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-edited instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn save_folder_instrument_writes_dsp_lisp_even_when_folder_is_new() {
        let name = format!("__test-agent-folder-{}/", std::process::id());
        let folder = std::path::Path::new(super::INSTRUMENTS_DIR).join(name.trim_end_matches('/'));
        let legacy_file = std::path::Path::new(super::INSTRUMENTS_DIR)
            .join(format!("{}.lisp", name.trim_end_matches('/')));
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&legacy_file);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_ui(&name, "(defsynth-ui (label \"ok\"))").unwrap();

        assert!(folder.join("dsp.lisp").exists());
        assert!(folder.join("ui.lisp").exists());
        assert!(
            !legacy_file.exists(),
            "folder-style saves must not fall back to legacy single-file instruments"
        );

        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&legacy_file);
    }

    #[test]
    fn missing_instrument_metadata_defaults_to_instrument_run_mode() {
        let name = format!("__test-run-mode-missing-{}/", std::process::id());
        let folder = std::path::Path::new(super::INSTRUMENTS_DIR).join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();

        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::Instrument
        );
        assert!(!folder.join("instrument.json").exists());

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn folder_instrument_run_mode_roundtrips_to_instrument_json() {
        let name = format!("__test-run-mode-folder-{}/", std::process::id());
        let folder = std::path::Path::new(super::INSTRUMENTS_DIR).join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::FreePatch).unwrap();

        let metadata_path = folder.join("instrument.json");
        assert_eq!(
            super::instrument_metadata_path(&name).unwrap(),
            metadata_path
        );
        assert!(metadata_path.exists());
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::FreePatch
        );
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::Instrument).unwrap();
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::Instrument
        );

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn legacy_file_instrument_run_mode_roundtrips_to_sidecar_json() {
        let name = format!("__test-run-mode-legacy-{}", std::process::id());
        let root = std::path::Path::new(super::INSTRUMENTS_DIR);
        let source_path = root.join(format!("{name}.lisp"));
        let metadata_path = root.join(format!("{name}.instrument.json"));
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&metadata_path);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::FreePatch).unwrap();

        assert_eq!(
            super::instrument_metadata_path(&name).unwrap(),
            metadata_path
        );
        assert!(metadata_path.exists());
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::FreePatch
        );

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&metadata_path);
    }

    #[test]
    fn invalid_instrument_run_mode_reports_error() {
        let name = format!("__test-run-mode-invalid-{}/", std::process::id());
        let folder = std::path::Path::new(super::INSTRUMENTS_DIR).join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        std::fs::write(
            folder.join("instrument.json"),
            r#"{ "version": 1, "run_mode": "forever_note" }"#,
        )
        .unwrap();

        let error = super::load_instrument_run_mode(&name).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("invalid instrument run_mode"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn moved_folder_instrument_resolves_by_unique_leaf_name() {
        let leaf = format!("__test-moved-folder-{}", std::process::id());
        let folder = std::path::Path::new(super::INSTRUMENTS_DIR)
            .join("__test-resolve-category")
            .join(&leaf);
        let direct_folder = std::path::Path::new(super::INSTRUMENTS_DIR).join(&leaf);
        let legacy_file = std::path::Path::new(super::INSTRUMENTS_DIR).join(format!("{leaf}.lisp"));
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
        let _ = std::fs::remove_dir_all(&direct_folder);
        let _ = std::fs::remove_file(&legacy_file);

        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();
        std::fs::write(folder.join("ui.lisp"), "(defsynth-ui (label \"ok\"))").unwrap();

        assert_eq!(
            super::instrument_source_path(&format!("{leaf}/")).unwrap(),
            folder.join("dsp.lisp")
        );
        assert_eq!(
            super::load_instrument_source(&format!("{leaf}/")).unwrap(),
            "(out 0 1 @name audio)"
        );
        assert_eq!(
            super::instrument_ui_path(&format!("{leaf}/")).unwrap(),
            folder.join("ui.lisp")
        );
        assert_eq!(
            super::load_instrument_ui_source(&format!("{leaf}/")).unwrap(),
            "(defsynth-ui (label \"ok\"))"
        );

        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
        let _ = std::fs::remove_dir_all(&direct_folder);
        let _ = std::fs::remove_file(&legacy_file);
    }

    #[test]
    fn moved_folder_instrument_leaf_match_requires_unique_source() {
        let leaf = format!("__test-ambiguous-folder-{}", std::process::id());
        let root = std::path::Path::new(super::INSTRUMENTS_DIR);
        let first = root.join("__test-ambiguous-a").join(&leaf);
        let second = root.join("__test-ambiguous-b").join(&leaf);
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-a"));
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-b"));

        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();
        std::fs::write(second.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();

        let error = super::instrument_source_path(&format!("{leaf}/")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains("Ambiguous instrument"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-a"));
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-b"));
    }

    #[test]
    fn instrument_preamble_uses_runtime_sample_rate_context() {
        let preamble = super::instrument_preamble(48_000);
        assert!(preamble.contains("runtime host sample-rate"));
        assert!(!preamble.contains("(def samplerate"));
        assert!(!preamble.contains("__SAMPLE_RATE__"));
    }

    #[test]
    fn effect_compile_injects_shared_preamble_helpers() {
        let source = r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param cutoff @default 1200 @min 40 @max 12000)
            (param q @default 0.8 @min 0.5 @max 4.0)
            (def filtered_l (svf in_l cutoff q 0))
            (def filtered_r (svf in_r cutoff q 0))
            (out filtered_l 1 @name left)
            (out filtered_r 2 @name right)
        "#;

        let result = super::compile_and_load(source, 44_100)
            .expect("effect compiler should inject shared preamble helpers");
        assert_eq!(result.manifest.n_inputs, 2);
        assert_eq!(result.manifest.n_outputs, 2);
    }

    #[test]
    fn custom_effect_mod_input_changes_mod_accessor_output_when_active() {
        let source = r#"
            (def input_l (in 1 @name Left))
            (def input_r (in 2 @name Right))
            (def mod1 (in 3 @name mod1 @modulator 1))
            (def mod2 (in 4 @name mod2 @modulator 2))
            (def mod3 (in 5 @name mod3 @modulator 3))
            (def mod4 (in 6 @name mod4 @modulator 4))
            (param xyz @default 0.25 @min 0.0 @max 1.0 @mod true @mod-mode additive)
            (def amount (mod xyz))
            (out (* input_l amount) 1 @name Left)
            (out (* input_r amount) 2 @name Right)
        "#;

        let render = |mod_value: f32| {
            super::render_effect_source_for_test(
                source,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 128,
                    frames: 2048,
                    param_overrides: vec![
                        ("__dgen_mod_active__xyz".to_string(), 1.0),
                        ("mod xyz slot 1 amt".to_string(), 0.5),
                    ],
                    input_overrides: vec![(2, mod_value)],
                },
            )
            .expect("effect should compile and render")
        };

        let unmodulated = render(0.0);
        let modulated = render(1.0);
        let diff = unmodulated
            .first_samples
            .iter()
            .zip(modulated.first_samples.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);

        assert!(
            diff > 0.01,
            "expected mod1 input to affect (mod xyz), diff={diff}"
        );
    }

    #[test]
    fn spectral_cumsum_soothe_amount_zero_full_wet_preserves_stereo_energy() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        for block_size in [256, 512] {
            let report = super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size,
                    frames: 8192,
                    param_overrides: vec![
                        ("amount".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render");
            println!("spectral-cumsum-soothe amount=0 mix=1 block={block_size} report: {report:?}");

            assert!(
                report.left_rms > 0.01,
                "left channel should not collapse at amount=0/mix=1 block={block_size}, report={report:?}"
            );
            assert!(
                report.right_rms > 0.01,
                "right channel should not collapse at amount=0/mix=1 block={block_size}, report={report:?}"
            );
            let ratio = report.left_rms / report.right_rms.max(1.0e-9);
            assert!(
                (0.25..4.0).contains(&ratio),
                "stereo energy should stay within a plausible range at block={block_size}, ratio={ratio}, report={report:?}"
            );
        }
    }

    #[test]
    fn spectral_cumsum_soothe_is_listed_and_ui_validates() {
        let effect_name = "spectral-cumsum-soothe";
        let listed = super::list_saved_effects();
        assert!(
            listed.iter().any(|name| name == effect_name),
            "effect picker list should include {effect_name:?}; listed={listed:?}"
        );

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let ui_source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("effects/spectral-cumsum-soothe/ui.lisp"),
        )
        .expect("read spectral cumsum soothe ui");
        crate::agent::ui_validate::validate_effect_ui_source(&ui_source, &compiled.manifest)
            .expect("effect ui should validate");
    }

    #[test]
    fn spectral_cumsum_soothe_high_amount_reduces_resonant_energy() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let render = |amount: f32| {
            super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("amount".to_string(), amount),
                        ("threshold".to_string(), 0.0),
                        ("attack".to_string(), 0.0),
                        ("release".to_string(), 0.8),
                        ("mix".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render")
        };

        let bypass = render(0.0);
        let active = render(8.0);
        println!("spectral-cumsum-soothe high amount bypass={bypass:?} active={active:?}");

        assert!(
            active.rms < bypass.rms * 0.85,
            "high amount should produce audible attenuation, bypass={bypass:?}, active={active:?}"
        );
        assert!(
            active.left_rms > 0.001 && active.right_rms > 0.001,
            "active processing should not collapse either channel, active={active:?}"
        );
    }

    #[test]
    fn spectral_cumsum_soothe_delta_is_removed_signal() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let render = |amount: f32| {
            super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("amount".to_string(), amount),
                        ("threshold".to_string(), 0.0),
                        ("gate".to_string(), -2.0),
                        ("low".to_string(), 0.0),
                        ("high".to_string(), 1.0),
                        ("attack".to_string(), 0.0),
                        ("release".to_string(), 0.8),
                        ("mix".to_string(), 1.0),
                        ("delta".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render")
        };

        let inactive = render(0.0);
        let active = render(8.0);
        println!("spectral-cumsum-soothe delta inactive={inactive:?} active={active:?}");

        assert!(
            inactive.rms < 0.001,
            "delta should be nearly silent when amount=0, inactive={inactive:?}"
        );
        assert!(
            active.rms > inactive.rms + 0.005,
            "delta should expose removed spectral energy under heavy reduction, inactive={inactive:?}, active={active:?}"
        );
    }

    #[test]
    fn spectral_notch_phaser_is_listed_and_ui_validates() {
        let effect_name = "spectral-notch-phaser";
        let listed = super::list_saved_effects();
        assert!(
            listed.iter().any(|name| name == effect_name),
            "effect picker list should include {effect_name:?}; listed={listed:?}"
        );

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-notch-phaser/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral notch phaser effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let ui_source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("effects/spectral-notch-phaser/ui.lisp"),
        )
        .expect("read spectral notch phaser ui");
        crate::agent::ui_validate::validate_effect_ui_source(&ui_source, &compiled.manifest)
            .expect("effect ui should validate");
    }

    #[test]
    fn spectral_notch_phaser_depth_changes_signal() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("effects/spectral-notch-phaser/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral notch phaser effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let render = |depth: f32| {
            super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("depth".to_string(), depth),
                        ("sharp".to_string(), 0.0),
                        ("distance".to_string(), 16.0),
                        ("lowkeep".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render")
        };

        let bypass = render(0.0);
        let active = render(1.0);
        println!("spectral-notch-phaser bypass={bypass:?} active={active:?}");

        assert!(
            active.rms < bypass.rms * 0.9,
            "deep notches should produce audible attenuation, bypass={bypass:?}, active={active:?}"
        );
        assert!(
            active.left_rms > 0.001 && active.right_rms > 0.001,
            "active processing should not collapse either channel, active={active:?}"
        );
    }

    #[test]
    fn dpro_wave_v2_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-wave-v2/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: Vec::new(),
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn digitone_bellington_high_note_remains_finite() {
        let name = "emulations/digitone/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let compile = super::compile_and_load_instrument_with_asset_base(
            &source,
            44_100,
            asset_base.as_deref(),
        )
        .unwrap();
        let preset = super::load_instrument_presets(name)
            .unwrap()
            .into_iter()
            .find(|preset| preset.name == "bellington")
            .expect("bellington preset should exist");
        let param_overrides = compile
            .manifest
            .params
            .iter()
            .filter_map(|param| {
                preset
                    .params
                    .get(&param.name)
                    .map(|value| (param.name.clone(), value.clamp(param.min, param.max)))
            })
            .collect();
        let report = super::render_loaded_instrument_for_test(
            &compile.manifest,
            &compile.lib,
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 44_100,
                midi_note: 153.0 + preset.base_note_offset,
                velocity: 1.0,
                gate_frames: 44_100,
                voice_index: 0,
                param_overrides,
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(
            report.non_finite_samples, 0,
            "expected finite output samples, got report: {report:?}"
        );
        assert_eq!(
            report.non_finite_state_slots, 0,
            "expected finite instrument state, got report: {report:?}"
        );
        assert!(
            report.peak.is_finite() && report.rms.is_finite() && report.mean_abs.is_finite(),
            "expected finite signal stats, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_ddrw_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-ddrw-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("wav1".to_string(), 4.0),
                    ("wav2".to_string(), 40.0),
                    ("mix".to_string(), 0.5),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_dens_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-dens-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("wave".to_string(), 16.0),
                    ("pch2".to_string(), 4.0),
                    ("pch3".to_string(), 7.0),
                    ("pch4".to_string(), 12.0),
                    ("chrl".to_string(), 0.35),
                    ("chrw".to_string(), 0.4),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_bbox_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-bbox-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 48.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("ptch".to_string(), 0.0),
                    ("start".to_string(), 0.0),
                    ("rtrg".to_string(), 0.0),
                    ("rtim".to_string(), 72.0),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn fmplus_stat_v1_renders_audible_signal() {
        let name = "emulations/monomachine-fmplus-stat-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("op1_frq".to_string(), 15.0),
                    ("op1_fin".to_string(), 0.0),
                    ("op1_fb".to_string(), 0.18),
                    ("op1_env".to_string(), 0.62),
                    ("op2_frq".to_string(), 19.0),
                    ("op2_vol".to_string(), 0.38),
                    ("tone".to_string(), 0.64),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn fmplus_par_v1_renders_audible_signal() {
        let name = "emulations/monomachine-fmplus-par-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("op1_frq".to_string(), 15.0),
                    ("op1_env".to_string(), 0.55),
                    ("op2_frq".to_string(), 19.0),
                    ("op2_env".to_string(), 0.42),
                    ("op3_frq".to_string(), 23.0),
                    ("op3_env".to_string(), 0.30),
                    ("op1_wave".to_string(), 18.0),
                    ("op1_mix".to_string(), 0.35),
                    ("op2_wave".to_string(), 34.0),
                    ("op2_mix".to_string(), 0.28),
                    ("op3_wave".to_string(), 51.0),
                    ("op3_mix".to_string(), 0.22),
                    ("car_wave".to_string(), 9.0),
                    ("car_mix".to_string(), 0.18),
                    ("tone".to_string(), 0.62),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }
}
