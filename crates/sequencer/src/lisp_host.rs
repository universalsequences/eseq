use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use eseqlisp::frame as eseq_frame;
use eseqlisp::parser::{ASTParser, Expression, Parser};
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
use crate::sequencer::{
    CustomInstrumentRunMode, PublishedSequencer, StepParam, StepSnapshot, Timebase,
};

pub mod dylib_cache;
mod graph_authoring;
mod graph_dsl;
mod graph_manifest;
mod graph_update;

pub use dylib_cache::{DGenCompileKind, DGenSourceOrigin, DylibLease};
pub use graph_authoring::register_graph_authoring_natives;
pub use graph_manifest::{graph_mode_present, parse_graph_manifest};
use graph_update::{CompiledGraphUpdate, SharedGraphNodeContext};

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

static MIDI_FX_DESCRIPTOR_CACHE: OnceLock<Mutex<HashMap<String, Vec<EffectDescriptor>>>> =
    OnceLock::new();

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

use crate::sequencer::MAX_INSTRUMENT_ENGINES;
pub const MAX_CUSTOM_FX: usize = 8;
pub const MAX_MIDI_FX_SLOTS: usize = 4;
pub const MAX_BUS_FX_CHAINS: usize = 64;

// ── Node state layout ──
// state[0] = host-local slot identity (diagnostics only)
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4] = enabled (0 = bypass/silent, 1 = active)
// state[5] = host sample rate
// state[6..10] = immutable process function pointer (four numeric u16 chunks)
// state[10..10+N] = DGenLisp read buffer
// state[...]     = DGenLisp write buffer (separate to respect `restrict`)

pub const DGEN_ENABLED_PARAM_IDX: usize = 4;
pub const DGEN_HOST_SAMPLE_RATE_IDX: usize = 5;
const DGEN_PROCESS_FN_START_IDX: usize = 6;
const DGEN_PROCESS_FN_CHUNKS: usize = 4;
pub const HEADER_SLOTS: usize = 10;
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

fn process_fn_pointer_chunks(process_fn: DGenProcessFn) -> [f32; DGEN_PROCESS_FN_CHUNKS] {
    let pointer = process_fn as usize as u64;
    std::array::from_fn(|chunk| ((pointer >> (chunk * 16)) & 0xffff) as f32)
}

unsafe fn dgen_process_fn_from_state(state: *mut f32) -> Option<DGenProcessFn> {
    let mut pointer = 0u64;
    for chunk in 0..DGEN_PROCESS_FN_CHUNKS {
        let value = *state.add(DGEN_PROCESS_FN_START_IDX + chunk);
        if !value.is_finite() || !(0.0..=u16::MAX as f32).contains(&value) {
            return None;
        }
        pointer |= (value as u16 as u64) << (chunk * 16);
    }
    (pointer != 0).then(|| std::mem::transmute::<usize, DGenProcessFn>(pointer as usize))
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
    if let Some(process_fn) = dgen_process_fn_from_state(s) {
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
///   [5..9] = process function pointer (four numeric u16 chunks)
///   [9] = num_entries (N)
///   [10..10+2N] = pairs of (index, value)
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
    for chunk in 0..DGEN_PROCESS_FN_CHUNKS {
        *dst.add(DGEN_PROCESS_FN_START_IDX + chunk) = *src.add(5 + chunk);
    }

    // Apply sparse index/value pairs into the memory region
    let num_entries = (*src.add(9)) as usize;
    let total_memory_slots = *dst.add(1) as usize;
    let mem = dgen_read_buffer_ptr(dst);
    for i in 0..num_entries {
        let idx = (*src.add(10 + i * 2)) as usize;
        let val = *src.add(10 + i * 2 + 1);
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
        ..NodeVTable::default()
    }
}

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

pub struct CompileResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub lease: Option<DylibLease>,
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
    compile_and_load_with_origin(source, sample_rate, asset_base, DGenSourceOrigin::Custom)
}

pub fn compile_and_load_with_origin(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    origin: DGenSourceOrigin,
) -> Result<CompileResult, String> {
    dylib_cache::global_cache_manager().acquire(
        DGenCompileKind::Effect,
        origin,
        source,
        sample_rate,
        asset_base,
    )
}

pub fn compile_and_load_uncached_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_lisp_with_asset_base(source, sample_rate, asset_base)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult {
        manifest,
        lib,
        lease: None,
    })
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

pub(crate) fn output_dir() -> PathBuf {
    std::env::temp_dir().join("sequencer_dgenlisp")
}

pub(crate) fn dgenlisp_tool_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp")
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
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("effect_{}", seq);
    let effective_source = effective_dgen_source(DGenCompileKind::Effect, source, sample_rate)?;
    compile_effective_dgen_source_to_dir(
        DGenCompileKind::Effect,
        &effective_source,
        sample_rate,
        asset_base,
        &dir,
        &dylib_name,
    )
}

pub(crate) fn materialize_defmacro_imports(source: &str) -> Result<String, String> {
    eseqlisp::defmacro_library::materialize_with_default_library(source)
        .map_err(|error| error.to_string())
}

pub(crate) fn effective_dgen_source(
    kind: DGenCompileKind,
    source: &str,
    sample_rate: u32,
) -> Result<String, String> {
    let source = materialize_defmacro_imports(source)?;
    let preamble = match kind {
        DGenCompileKind::Effect => effect_preamble(sample_rate),
        DGenCompileKind::Instrument => instrument_preamble(sample_rate),
    };
    Ok(format!("{preamble}\n\n{source}"))
}

pub(crate) fn compile_effective_dgen_source_to_dir(
    kind: DGenCompileKind,
    effective_source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    dir: &Path,
    dylib_name: &str,
) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create output dir: {e}"))?;
    let source_name = match kind {
        DGenCompileKind::Effect => "effect",
        DGenCompileKind::Instrument => "instrument",
    };
    let src_path = dir.join(format!("{dylib_name}.lisp"));
    std::fs::write(&src_path, effective_source)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = dgenlisp_tool_path();
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()]);
    if kind == DGenCompileKind::Instrument {
        command.args(["--voices", "12"]);
    }
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
        log_dgenlisp_compile_failure(source_name, &src_path, &error, effective_source);
        return Err(error);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    log_dgenlisp_compile_manifest(source_name, &src_path, &stdout);
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
/// [slot_id, total_memory_slots, canary, declared_input_count, enabled,
///  process_fn_chunk0..3, num_entries, idx0, val0, ...]
/// The engine zeroes state; init only needs to set non-zero values.
fn build_init_message(
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

unsafe fn disconnect_direct_chain(
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
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub key_locks: std::collections::BTreeMap<u8, std::collections::BTreeMap<String, f32>>,
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

const INSTRUMENT_REGISTRY_SIZE: usize = MAX_INSTRUMENT_ENGINES * MAX_VOICES;
static DGEN_INSTRUMENT_FNS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
static DGEN_INSTRUMENT_OUTPUT_COUNTS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
static DGEN_ENGINE_ENABLED_VOICES: [AtomicUsize; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static DGEN_ENGINE_PROCESS_CALLS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static DGEN_ENGINE_PROCESS_BLOCKS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
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
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id].store(count.min(MAX_VOICES), Ordering::Release);
    }
}

pub fn get_dgen_engine_enabled_voices(engine_id: usize) -> usize {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .min(MAX_VOICES)
    } else {
        1
    }
}

pub fn reset_dgen_engine_enabled_voices(engine_id: usize) {
    set_dgen_engine_enabled_voices(engine_id, 1);
}

pub fn take_dgen_engine_process_stats() -> Vec<DGenEngineProcessStats> {
    (0..MAX_INSTRUMENT_ENGINES)
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
    if engine_id < MAX_INSTRUMENT_ENGINES {
        let enabled = DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .min(MAX_VOICES);
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
        if engine_id < MAX_INSTRUMENT_ENGINES {
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
        ..NodeVTable::default()
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

    // Header (10) + pairs (2 * N). Instrument nodes resolve their process
    // function through DGEN_INSTRUMENT_FNS, so the pointer chunks stay zero.
    let mut msg = Vec::with_capacity(10 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    msg.extend([0.0; DGEN_PROCESS_FN_CHUNKS]);
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

fn validate_instrument_relative_dir(path: &str) -> io::Result<PathBuf> {
    let trimmed = path.trim().trim_matches('/');
    let mut relative = PathBuf::new();
    if trimmed.is_empty() {
        return Ok(relative);
    }
    for component in Path::new(trimmed).components() {
        match component {
            std::path::Component::Normal(part) => relative.push(part),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid instrument folder '{path}'"),
                ));
            }
        }
    }
    Ok(relative)
}

pub fn move_saved_instrument(name: &str, target_folder: &str) -> io::Result<String> {
    let root = Path::new(INSTRUMENTS_DIR);
    let source = instrument_source_path(name)?;
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }

    let target_dir = root.join(validate_instrument_relative_dir(target_folder)?);
    if !target_dir.exists() || !target_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "target instrument folder '{}' does not exist",
                target_dir.display()
            ),
        ));
    }

    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        let source_dir = source.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "instrument source '{}' has no parent directory",
                    source.display()
                ),
            )
        })?;
        if source_dir == target_dir {
            return Ok(name.to_string());
        }
        if target_dir.starts_with(source_dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot move an instrument folder into itself",
            ));
        }
        let folder_name = source_dir.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument folder '{}' has no name", source_dir.display()),
            )
        })?;
        let dest = target_dir.join(folder_name);
        if dest.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("target instrument '{}' already exists", dest.display()),
            ));
        }
        std::fs::rename(source_dir, &dest)?;
        return dest
            .strip_prefix(root)
            .map(|rel| format!("{}/", rel.to_string_lossy().replace('\\', "/")))
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()));
    }

    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument source '{}' has no file stem", source.display()),
            )
        })?;
    let dest_source = target_dir.join(format!("{stem}.lisp"));
    if dest_source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "target instrument '{}' already exists",
                dest_source.display()
            ),
        ));
    }
    let mut sidecars = Vec::new();
    for extension in ["presets", "instrument.json"] {
        let sidecar = source.with_extension(extension);
        if sidecar.exists() {
            let dest = target_dir.join(sidecar.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("sidecar '{}' has no file name", sidecar.display()),
                )
            })?);
            if dest.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("target sidecar '{}' already exists", dest.display()),
                ));
            }
            sidecars.push((sidecar, dest));
        }
    }
    std::fs::rename(&source, &dest_source)?;
    for (sidecar, dest) in sidecars {
        std::fs::rename(sidecar, dest)?;
    }
    dest_source
        .strip_prefix(root)
        .map(|rel| rel.with_extension("").to_string_lossy().replace('\\', "/"))
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
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
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("instrument_{}", seq);
    let effective_source = effective_dgen_source(DGenCompileKind::Instrument, source, sample_rate)?;
    compile_effective_dgen_source_to_dir(
        DGenCompileKind::Instrument,
        &effective_source,
        sample_rate,
        asset_base,
        &dir,
        &dylib_name,
    )
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
    compile_and_load_instrument_with_origin(
        source,
        sample_rate,
        asset_base,
        DGenSourceOrigin::Custom,
    )
}

pub fn compile_and_load_instrument_with_origin(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    origin: DGenSourceOrigin,
) -> Result<CompileResult, String> {
    dylib_cache::global_cache_manager().acquire(
        DGenCompileKind::Instrument,
        origin,
        source,
        sample_rate,
        asset_base,
    )
}

pub fn compile_and_load_instrument_uncached_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_instrument_with_asset_base(source, sample_rate, asset_base)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult {
        manifest,
        lib,
        lease: None,
    })
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
    let entry_count = init_msg.get(9).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[10 + i * 2] as usize;
        let value = init_msg[10 + i * 2 + 1];
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
    let init_msg = build_init_message(0, manifest, None);
    let entry_count = init_msg.get(9).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[10 + i * 2] as usize;
        let value = init_msg[10 + i * 2 + 1];
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
    pub lease: Option<DylibLease>,
    pub source: String,
    pub params: Vec<DGenParam>,
    pub name: String,
}

pub struct EffectEditResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub lease: Option<DylibLease>,
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
type SharedProcessAuthoring = Arc<Mutex<ProcessAuthoringRegistry>>;
type SharedProcessEvalContext = Arc<Mutex<Option<ProcessEvalContext>>>;
type ProcessPublishHook = Arc<dyn Fn(crate::process::PublishedProcessAuthoringSnapshot) + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessEvalScope {
    Run,
    RatchetShape,
}

#[derive(Clone)]
pub struct PublishedProcessAuthoringNatives {
    process_authoring: SharedProcessAuthoring,
    process_chain_state: Arc<crate::sequencer::SequencerState>,
    publish: Option<ProcessPublishHook>,
}

impl PublishedProcessAuthoringNatives {
    pub fn define_process_accumulator(
        &self,
        args: Vec<eseqlisp::vm::Value>,
        vm: &mut eseqlisp::vm::VM,
    ) -> Result<eseqlisp::vm::Value, String> {
        register_process_accumulator_def(
            args,
            vm,
            &self.process_authoring,
            Some(Arc::clone(&self.process_chain_state)),
            self.publish.clone(),
        )
    }
}
const UI_PROCESS_HANDLE_BASE: u64 = 1_u64 << 48;
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

#[derive(Default)]
struct ProcessAuthoringRegistry {
    next_handle_id: u64,
    defs: Vec<crate::process::ProcessDef>,
    instances: Vec<crate::process::AuthoredProcessInstance>,
    channels: Vec<crate::process::AuthoredChannel>,
    patches: Vec<crate::process::AuthoredPatch>,
    conductors: Vec<crate::process::AuthoredConductorAttachment>,
    outlet_handles: HashMap<u64, crate::process::ProcessOutletRef>,
    channel_handles: HashMap<u64, String>,
}

impl ProcessAuthoringRegistry {
    fn with_handle_base(handle_base: u64) -> Self {
        Self {
            next_handle_id: handle_base,
            ..Self::default()
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_handle_id = self.next_handle_id.saturating_add(1).max(1);
        self.next_handle_id
    }

    fn upsert_def(&mut self, def: crate::process::ProcessDef) {
        if let Some(existing) = self.defs.iter_mut().find(|entry| entry.id == def.id) {
            *existing = def;
        } else {
            self.defs.push(def);
        }
    }

    fn upsert_instance(&mut self, instance: crate::process::AuthoredProcessInstance) {
        if let Some(existing) = self
            .instances
            .iter_mut()
            .find(|entry| entry.handle_id == instance.handle_id)
        {
            *existing = instance;
        } else {
            self.instances.push(instance);
        }
    }

    fn name_instance(&mut self, handle_id: crate::process::AuthoredHandleId, name: &str) {
        self.instances
            .retain(|entry| entry.handle_id == handle_id || entry.name.as_deref() != Some(name));
        if let Some(instance) = self
            .instances
            .iter_mut()
            .find(|entry| entry.handle_id == handle_id)
        {
            instance.name = Some(name.to_string());
            instance.anonymous = false;
        }
    }

    fn name_channel(&mut self, handle_id: crate::process::AuthoredHandleId, name: &str) {
        self.channels
            .retain(|entry| entry.handle_id == handle_id || entry.name.as_deref() != Some(name));
        if let Some(channel) = self
            .channels
            .iter_mut()
            .find(|entry| entry.handle_id == handle_id)
        {
            channel.name = Some(name.to_string());
        }
    }

    fn snapshot(&self) -> crate::process::ProcessAuthoringSnapshot {
        let live_handles = self
            .instances
            .iter()
            .map(|instance| instance.handle_id)
            .collect::<HashSet<_>>();
        crate::process::ProcessAuthoringSnapshot {
            defs: self.defs.clone(),
            instances: self.instances.clone(),
            channels: self.channels.clone(),
            patches: self.patches.clone(),
            conductors: self
                .conductors
                .iter()
                .filter(|attachment| live_handles.contains(&attachment.process_handle_id))
                .cloned()
                .collect(),
        }
    }
}

pub(crate) struct ProcessEvalContext {
    runtime_id: u64,
    beat: f64,
    inlets: HashMap<String, EValue>,
    state: HashMap<String, EValue>,
    event: Option<EValue>,
    step_context: Option<crate::process::ProcessStepEventContext>,
    ports: Vec<crate::process::ProcessPortDef>,
    reads: crate::process::ProcessReadSnapshot,
    conductor_observe_tracks: Vec<usize>,
    conductor_play_tracks: Vec<usize>,
    outputs: Vec<crate::process::ProcessOutput>,
    emissions: Vec<EmittedAccumulatorEvent>,
    commands: Vec<crate::process::ProcessRunCommand>,
    target_writes: Vec<crate::process::ProcessTargetWrite>,
    transpose: Option<f32>,
    random_state: u64,
    scope: ProcessEvalScope,
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
    process_authoring: SharedProcessAuthoring,
    process_eval: SharedProcessEvalContext,
    graph_node: SharedGraphNodeContext,
    graph_updates: HashMap<u64, CompiledGraphUpdate>,
    process_run_callbacks: HashMap<String, EValue>,
    #[cfg(test)]
    process_run_cache_enabled: bool,
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
        Self::new_with_process_chain_writes(
            state,
            effect_descriptors,
            instrument_descriptors,
            track,
            cursor_step,
            true,
        )
    }

    pub fn new_scheduler(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
    ) -> Self {
        Self::new_with_process_chain_writes(
            state,
            effect_descriptors,
            instrument_descriptors,
            track,
            cursor_step,
            false,
        )
    }

    fn new_with_process_chain_writes(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
        write_process_chain_state: bool,
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
        let process_authoring = Arc::new(Mutex::new(ProcessAuthoringRegistry::default()));
        let process_eval = Arc::new(Mutex::new(None));
        let graph_node: SharedGraphNodeContext = Arc::new(Mutex::new(None));
        let process_chain_state = write_process_chain_state.then(|| Arc::clone(&state));
        let mut runtime = Runtime::new();
        runtime.set_theme_sync_enabled(false);
        runtime.register_native_with_docs(
            "seq-register-script-source-tab",
            "(seq-register-script-source-tab label)",
            "No-op outside the Metal Seq UI; lets source-tab scripts load in scratch/scheduler runtimes.",
            |_args, _ctx| Ok(EValue::Nil),
        );
        register_sequencer_natives_with_accumulators(
            &mut runtime,
            Arc::clone(&state),
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
        register_process_natives(
            &mut runtime,
            Arc::clone(&process_authoring),
            Arc::clone(&process_eval),
            None,
            process_chain_state.clone(),
            true,
        );
        register_process_chain_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&process_authoring),
            None,
            write_process_chain_state,
        );
        register_def_accumulator_dispatch_native(
            &mut runtime,
            Arc::clone(&accumulators),
            Arc::clone(&process_authoring),
            process_chain_state,
            None,
        );
        graph_update::register_graph_node_natives(&mut runtime, Arc::clone(&graph_node));
        register_process_graph_emit_native(&mut runtime, Arc::clone(&process_eval));
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
            process_authoring,
            process_eval,
            graph_node,
            graph_updates: HashMap::new(),
            process_run_callbacks: HashMap::new(),
            #[cfg(test)]
            process_run_cache_enabled: true,
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
        // External evaluation can redefine macros used by a shipped process
        // body without changing that body's source text. Recompile callbacks
        // after every authoring evaluation so cached macro expansion can never
        // outlive the environment that produced it.
        self.process_run_callbacks.clear();
        self.runtime.eval_str(code).map_err(|e| format!("{e:?}"))
    }

    pub fn eval_source_at_path(
        &mut self,
        path: impl Into<PathBuf>,
        code: &str,
    ) -> Result<Option<EValue>, String> {
        self.process_run_callbacks.clear();
        self.runtime
            .eval_source_at_path(path.into(), code)
            .map_err(|e| format!("{e:?}"))
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

    fn install_accumulator_macro(&mut self) {}

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
                    tensor_params: Vec::new(),
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
        SharedProcessAuthoring,
        SharedProcessEvalContext,
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
            self.process_authoring,
            self.process_eval,
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
        process_authoring: SharedProcessAuthoring,
        process_eval: SharedProcessEvalContext,
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
            process_authoring,
            process_eval,
            graph_node,
            graph_updates: HashMap::new(),
            process_run_callbacks: HashMap::new(),
            #[cfg(test)]
            process_run_cache_enabled: true,
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
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

    pub fn process_authoring_snapshot(&self) -> crate::process::ProcessAuthoringSnapshot {
        self.process_authoring
            .lock()
            .map(|registry| registry.snapshot())
            .unwrap_or_default()
    }

    pub fn invoke_process_run(
        &mut self,
        invocation: crate::process::ProcessRunInvocation,
    ) -> Result<crate::process::ProcessRunResult, String> {
        let conductor_observe_tracks = invocation.reads.conductor_observe_tracks.clone();
        let conductor_play_tracks = invocation.reads.conductor_play_tracks.clone();
        {
            let mut ctx = self
                .process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            *ctx = Some(ProcessEvalContext {
                runtime_id: invocation.runtime_id,
                beat: invocation.beat,
                inlets: invocation.inlets,
                state: invocation.state,
                event: invocation.event,
                step_context: invocation.step_context,
                ports: invocation.ports,
                reads: invocation.reads,
                conductor_observe_tracks,
                conductor_play_tracks,
                outputs: Vec::new(),
                emissions: Vec::new(),
                commands: Vec::new(),
                target_writes: Vec::new(),
                transpose: None,
                random_state: invocation.seed,
                scope: ProcessEvalScope::Run,
            });
        }
        let _ = self.runtime.take_status_message();
        if self.process_run_cache_enabled() {
            let callback =
                if let Some(callback) = self.process_run_callbacks.get(&invocation.source) {
                    callback.clone()
                } else {
                    let callback_source = format!("(lambda () {})", invocation.source);
                    let callback = self
                        .runtime
                        .eval_str(&callback_source)
                        .map_err(|error| format!("{error:?}"))?
                        .ok_or_else(|| "process body did not compile to a callback".to_string())?;
                    self.process_run_callbacks
                        .insert(invocation.source.clone(), callback.clone());
                    callback
                };
            self.runtime
                .invoke(callback, Vec::new())
                .map_err(|error| format!("{error:?}"))?;
        } else {
            self.runtime
                .eval_str(&invocation.source)
                .map_err(|error| format!("{error:?}"))?;
        }
        let process_status = self.runtime.take_status_message();
        let ctx = self
            .process_eval
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?
            .take()
            .ok_or_else(|| "process run did not produce a context".to_string())?;
        if let Some(status) = process_status {
            if status.starts_with("Error:") {
                return Err(status);
            }
        }
        Ok(crate::process::ProcessRunResult {
            runtime_id: invocation.runtime_id,
            beat: invocation.beat,
            sample_time: invocation.sample_time,
            state: ctx.state,
            outputs: ctx.outputs,
            emissions: ctx.emissions,
            commands: ctx.commands,
            target_writes: ctx.target_writes,
            transpose: ctx.transpose,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_process_run_cache_enabled(&mut self, enabled: bool) {
        self.process_run_cache_enabled = enabled;
    }

    #[inline]
    fn process_run_cache_enabled(&self) -> bool {
        #[cfg(test)]
        {
            self.process_run_cache_enabled
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    pub fn invoke_process_ratchet_shape(
        &mut self,
        shape_context: &mut crate::process::ProcessRatchetShapeContext,
        shape: &EValue,
        index: u32,
        event: crate::process::ProcessRatchetEvent,
    ) -> Result<crate::process::ProcessRatchetEvent, String> {
        let event_value = process_ratchet_event_value(event);
        {
            let mut ctx = self
                .process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            *ctx = Some(ProcessEvalContext {
                runtime_id: shape_context.runtime_id,
                beat: shape_context.beat,
                inlets: shape_context.inlets.clone(),
                state: shape_context.state.clone(),
                event: shape_context.event.clone(),
                step_context: Some(shape_context.step_context.clone()),
                ports: shape_context.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                conductor_observe_tracks: Vec::new(),
                conductor_play_tracks: Vec::new(),
                outputs: Vec::new(),
                emissions: Vec::new(),
                commands: Vec::new(),
                target_writes: Vec::new(),
                transpose: None,
                random_state: shape_context.random_state,
                scope: ProcessEvalScope::RatchetShape,
            });
        }
        let _ = self.runtime.take_status_message();
        let invoke_result = self.runtime.invoke(
            shape.clone(),
            vec![EValue::Number(index as f64), event_value.clone()],
        );
        let shape_status = self.runtime.take_status_message();
        let ctx = self
            .process_eval
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?
            .take()
            .ok_or_else(|| "ratchet shape did not produce an evaluation context".to_string())?;
        shape_context.random_state = ctx.random_state;
        if let Some(status) = shape_status {
            if status.starts_with("Error:") {
                return Err(status);
            }
        }
        let returned = invoke_result.map_err(|error| format!("{error:?}"))?;
        let shaped_value = match returned {
            Some(value @ EValue::Map(_)) => value,
            _ => event_value,
        };
        process_ratchet_event_from_value(&shaped_value)
    }
}

fn publish_process_authoring(
    process_authoring: &SharedProcessAuthoring,
    publish: &Option<ProcessPublishHook>,
) {
    let Some(publish) = publish else {
        return;
    };
    if let Ok(registry) = process_authoring.lock() {
        match registry.snapshot().to_published() {
            Ok(snapshot) => publish(snapshot),
            Err(error) => eprintln!("[process] publish error: {error}"),
        }
    }
}

fn register_process_target_hint_constructors(runtime: &mut Runtime) {
    for (name, arity) in [
        ("step-param", 1usize),
        ("param-tag", 1),
        ("instrument-param", 1),
        ("effect-param", 2),
        ("midi-fx-target", 2),
    ] {
        runtime.register_native_with_docs(
            name,
            name,
            "Construct a process target hint expression.",
            move |args, _ctx| {
                if args.len() != arity {
                    return Err(format!("{name} expects {arity} argument(s)"));
                }
                Ok(process_list(
                    std::iter::once(EValue::Symbol(name.to_string())).chain(args.into_iter()),
                ))
            },
        );
    }
    runtime.register_native_with_docs(
        "process-inlet",
        "(process-inlet :class :inlet)",
        "Construct a process-inlet target selector for a connectable process port.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("process-inlet expects a process class and inlet".to_string());
            }
            Ok(process_list(
                std::iter::once(EValue::Symbol("process-inlet".to_string()))
                    .chain(args.into_iter()),
            ))
        },
    );
}

fn register_process_natives(
    runtime: &mut Runtime,
    process_authoring: SharedProcessAuthoring,
    process_eval: SharedProcessEvalContext,
    publish: Option<ProcessPublishHook>,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    register_execution_natives: bool,
) {
    let process_authoring_for_inline_metadata = Arc::clone(&process_authoring);
    runtime.set_inline_widget_metadata_resolver(Rc::new(move |callee, inlet| {
        let registry = process_authoring_for_inline_metadata.lock().ok()?;
        let inlet = registry
            .defs
            .iter()
            .find(|definition| definition.name == callee)?
            .inlets
            .iter()
            .find(|definition| definition.name == inlet)?;
        let step = matches!(
            inlet.kind,
            crate::process::ProcessInletKind::Int
                | crate::process::ProcessInletKind::Gate
                | crate::process::ProcessInletKind::Track
        )
        .then_some(1.0);
        Some(eseqlisp::vm::InlineWidgetMetadata {
            min: inlet.min.map(f64::from),
            max: inlet.max.map(f64::from),
            step,
        })
    }));
    let process_authoring_for_hook = Arc::clone(&process_authoring);
    let publish_for_hook = publish.clone();
    runtime.add_global_store_hook(Rc::new(move |name, value| {
        let EValue::HostHandle { kind, id, .. } = value else {
            return;
        };
        let handle_id = crate::process::AuthoredHandleId(*id);
        if let Ok(mut registry) = process_authoring_for_hook.lock() {
            match kind.as_str() {
                "process" => registry.name_instance(handle_id, name),
                "channel" => registry.name_channel(handle_id, name),
                _ => {}
            }
        }
        publish_process_authoring(&process_authoring_for_hook, &publish_for_hook);
    }));

    register_process_target_hint_constructors(runtime);

    let process_authoring_for_inlet = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "inlet",
        "(inlet process :inlet)",
        "Construct an instance-specific process-inlet target selector for connect!.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("inlet expects a process handle and inlet name".to_string());
            }
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("inlet expects a process handle".to_string());
            };
            if kind != "process" {
                return Err("inlet expects a process handle".to_string());
            }
            let inlet = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "inlet expects an inlet name".to_string())?,
            )?;
            let registry = process_authoring_for_inlet
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let instance = registry
                .instances
                .iter()
                .find(|entry| entry.handle_id.0 == *id)
                .ok_or_else(|| "unknown process handle".to_string())?;
            let def = registry
                .defs
                .iter()
                .find(|def| def.name == instance.class_name)
                .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
            if !def.inlets.iter().any(|entry| entry.name == inlet) {
                return Err(format!(
                    "process '{}' has no inlet '{}'",
                    instance.class_name, inlet
                ));
            }
            Ok(process_list([
                EValue::Symbol(PROCESS_INLET_INSTANCE_TARGET_TAG.to_string()),
                EValue::Number(*id as f64),
                EValue::Symbol(instance.class_name.clone()),
                EValue::Symbol(inlet),
            ]))
        },
    );

    let process_authoring_for_def = Arc::clone(&process_authoring);
    let publish_for_def = publish.clone();
    let chain_state_for_def = process_chain_state.clone();
    runtime.register_vm_native_with_docs(
        "def-process",
        "(def-process name :in (...) :out (...) :state (...) :every (beats n) :run body)",
        "Define a scheduler-side musical process class.",
        move |args, vm| match register_process_def(
            args,
            vm,
            &process_authoring_for_def,
            chain_state_for_def.clone(),
            publish_for_def.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[process] def-process error: {error}");
                EValue::Bool(false)
            }
        },
    );

    let process_authoring_for_def_acc = Arc::clone(&process_authoring);
    let publish_for_def_acc = publish.clone();
    let chain_state_for_def_acc = process_chain_state.clone();
    runtime.register_vm_native_with_docs(
        "def-accumulator",
        "(def-accumulator name :target (step-param :transpose) :amount (...) :reset :lane :range (lo hi) :mode :wrap)",
        "Define a replay-safe lane-folding step process accumulator.",
        move |args, vm| match register_process_accumulator_def(
            args,
            vm,
            &process_authoring_for_def_acc,
            chain_state_for_def_acc.clone(),
            publish_for_def_acc.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[process] def-accumulator error: {error}");
                EValue::Bool(false)
            }
        },
    );

    let process_authoring_for_defchan = Arc::clone(&process_authoring);
    let publish_for_defchan = publish.clone();
    runtime.register_native_with_docs(
        "defchan",
        "(defchan name [initial])",
        "Declare a late-bound musical value/message channel.",
        move |args, _ctx| {
            let name = process_symbol_name(
                args.first()
                    .ok_or_else(|| "defchan expects a channel name".to_string())?,
            )?;
            let mut registry = process_authoring_for_defchan
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let handle_id = crate::process::AuthoredHandleId(registry.next_id());
            let initial = args.get(1).cloned();
            registry.channels.push(crate::process::AuthoredChannel {
                handle_id,
                name: Some(name.clone()),
                initial,
                message_only: args.len() == 1,
            });
            registry.channel_handles.insert(handle_id.0, name);
            let handle =
                process_channel_handle(Arc::clone(&process_authoring_for_defchan), handle_id);
            drop(registry);
            publish_process_authoring(&process_authoring_for_defchan, &publish_for_defchan);
            Ok(handle)
        },
    );

    let process_authoring_for_start = Arc::clone(&process_authoring);
    let publish_for_start = publish.clone();
    runtime.register_native_with_docs(
        "start",
        "(start process)",
        "Start a process instance.",
        move |args, _ctx| {
            set_process_running(&process_authoring_for_start, args.first(), true)?;
            publish_process_authoring(&process_authoring_for_start, &publish_for_start);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_stop = Arc::clone(&process_authoring);
    let publish_for_stop = publish.clone();
    runtime.register_native_with_docs(
        "stop",
        "(stop process)",
        "Stop a process instance.",
        move |args, _ctx| {
            set_process_running(&process_authoring_for_stop, args.first(), false)?;
            publish_process_authoring(&process_authoring_for_stop, &publish_for_stop);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_every = Arc::clone(&process_authoring);
    let publish_for_every = publish.clone();
    let chain_state_for_every = process_chain_state.clone();
    runtime.register_native_with_docs(
        "every",
        "(every time body...)",
        "Create and start an anonymous process that runs on a quantized musical interval.",
        move |args, _ctx| {
            let interval = args
                .first()
                .ok_or_else(|| "every expects a time expression".to_string())
                .and_then(parse_process_time_expr)?;
            let run_source = process_body_source(
                args.get(1..)
                    .ok_or_else(|| "every expects a body".to_string())?,
            )?;
            let handle = construct_process_instance(
                &process_authoring_for_every,
                "__anonymous_every",
                Vec::new(),
                true,
                true,
                Some(interval),
                Some(run_source),
                false,
                chain_state_for_every.clone(),
                publish_for_every.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_every, &publish_for_every);
            Ok(handle)
        },
    );

    let process_authoring_for_after = Arc::clone(&process_authoring);
    let publish_for_after = publish.clone();
    let chain_state_for_after = process_chain_state.clone();
    runtime.register_native_with_docs(
        "after",
        "(after time body...)",
        "Create and start an anonymous one-shot process that runs after a musical delay.",
        move |args, _ctx| {
            let delay = args
                .first()
                .ok_or_else(|| "after expects a time expression".to_string())
                .and_then(parse_process_time_expr)?;
            let run_source = process_body_source(
                args.get(1..)
                    .ok_or_else(|| "after expects a body".to_string())?,
            )?;
            let handle = construct_process_instance(
                &process_authoring_for_after,
                "__anonymous_after",
                Vec::new(),
                true,
                true,
                Some(delay),
                Some(run_source),
                true,
                chain_state_for_after.clone(),
                publish_for_after.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_after, &publish_for_after);
            Ok(handle)
        },
    );

    let process_authoring_for_on = Arc::clone(&process_authoring);
    let publish_for_on = publish.clone();
    runtime.register_native_with_docs(
        "on",
        "(on source callback)",
        "Create and start an anonymous process that runs when an event source fires.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("on expects source and callback".to_string());
            }
            let source = parse_process_event_source_ref(&process_authoring_for_on, &args[0])?;
            let handle = construct_anonymous_listener_process(
                &process_authoring_for_on,
                "on",
                source,
                &args[1],
                publish_for_on.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_on, &publish_for_on);
            Ok(handle)
        },
    );

    let process_authoring_for_patch = Arc::clone(&process_authoring);
    let publish_for_patch = publish.clone();
    runtime.register_native_with_docs(
        "patch",
        "(patch source target)",
        "Connect a process outlet/channel source to a process inlet/channel target.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("patch expects source and target".to_string());
            }
            let source = parse_process_source_ref(&process_authoring_for_patch, &args[0])?;
            let target = parse_process_target_ref(&process_authoring_for_patch, &args[1])?;
            process_authoring_for_patch
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?
                .patches
                .push(crate::process::AuthoredPatch { source, target });
            publish_process_authoring(&process_authoring_for_patch, &publish_for_patch);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_tap = Arc::clone(&process_authoring);
    let publish_for_tap = publish.clone();
    runtime.register_native_with_docs(
        "tap",
        "(tap source callback)",
        "Create and start an anonymous process that runs whenever a source publishes.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("tap expects source and callback".to_string());
            }
            let source = parse_process_event_source_ref(&process_authoring_for_tap, &args[0])?;
            let handle = construct_anonymous_listener_process(
                &process_authoring_for_tap,
                "tap",
                source,
                &args[1],
                publish_for_tap.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_tap, &publish_for_tap);
            Ok(handle)
        },
    );

    let process_authoring_for_send = Arc::clone(&process_authoring);
    let process_eval_for_send = Arc::clone(&process_eval);
    let publish_for_send = publish.clone();
    runtime.register_native_with_docs(
        "send",
        "(send channel value)",
        "Publish a value/message to a process channel.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("send expects channel and value".to_string());
            }
            let channel = parse_channel_name(&process_authoring_for_send, &args[0])?;
            let value = args[1].clone();
            let mut eval = process_eval_for_send
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            if let Some(ctx) = eval.as_mut() {
                ensure_process_run_scope(ctx, "send")?;
                // Runtime propagation is performed by the scheduler after the run
                // result returns.
                ctx.outputs.push(crate::process::ProcessOutput {
                    name: format!("__chan:{channel}"),
                    value: value.clone(),
                });
            } else {
                drop(eval);
                let mut registry = process_authoring_for_send
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                if let Some(authored) = registry
                    .channels
                    .iter_mut()
                    .rev()
                    .find(|authored| authored.name.as_deref() == Some(channel.as_str()))
                {
                    if !authored.message_only {
                        authored.initial = Some(value.clone());
                    }
                }
                drop(registry);
                publish_process_authoring(&process_authoring_for_send, &publish_for_send);
            }
            Ok(value)
        },
    );

    if !register_execution_natives {
        let process_authoring_for_ps = Arc::clone(&process_authoring);
        runtime.register_native_with_docs(
            "ps",
            "(ps)",
            "Return authored process/channel status.",
            move |_args, _ctx| {
                let registry = process_authoring_for_ps
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                Ok(process_status_value(&registry))
            },
        );
        return;
    }

    let process_eval_for_in = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "in",
        "(in :name)",
        "Read a process inlet.",
        move |args, _ctx| {
            let key = process_key_arg(args.first(), "in")?;
            let guard = process_eval_for_in
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("in called outside process execution".to_string());
            };
            Ok(ctx.inlets.get(&key).cloned().unwrap_or(EValue::Nil))
        },
    );

    let process_eval_for_out = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "out",
        "(out :name value)",
        "Publish a process outlet value.",
        move |args, _ctx| {
            let key = process_key_arg(args.first(), "out")?;
            let value = args.get(1).cloned().unwrap_or(EValue::Nil);
            let mut guard = process_eval_for_out
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("out called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "out")?;
            ctx.outputs.push(crate::process::ProcessOutput {
                name: key,
                value: value.clone(),
            });
            Ok(value)
        },
    );

    let process_eval_for_state_get = Arc::clone(&process_eval);
    runtime.register_native("__process-state-get", move |args, _ctx| {
        let key = process_key_arg(args.first(), "__process-state-get")?;
        let guard = process_eval_for_state_get
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_ref() else {
            return Err("__process-state-get called outside process execution".to_string());
        };
        Ok(ctx.state.get(&key).cloned().unwrap_or(EValue::Nil))
    });

    let process_eval_for_state_set = Arc::clone(&process_eval);
    runtime.register_native("__process-state-set!", move |args, _ctx| {
        let key = process_key_arg(args.first(), "__process-state-set!")?;
        let value = args.get(1).cloned().unwrap_or(EValue::Nil);
        let mut guard = process_eval_for_state_set
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_mut() else {
            return Err("__process-state-set! called outside process execution".to_string());
        };
        ensure_process_run_scope(ctx, "__process-state-set!")?;
        ctx.state.insert(key, value.clone());
        Ok(value)
    });

    let process_eval_for_event = Arc::clone(&process_eval);
    runtime.register_native("__process-event-value", move |_args, _ctx| {
        let guard = process_eval_for_event
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_ref() else {
            return Err("__process-event-value called outside process execution".to_string());
        };
        Ok(ctx.event.clone().unwrap_or(EValue::Nil))
    });

    let process_eval_for_transpose = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "transpose!",
        "(transpose! semitones)",
        "Set scheduler global transpose for future note-ons.",
        move |args, _ctx| {
            let Some(EValue::Number(value)) = args.first() else {
                return Err("transpose! expects a number".to_string());
            };
            let mut guard = process_eval_for_transpose
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("transpose! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "transpose!")?;
            ctx.transpose = Some(*value as f32);
            Ok(EValue::Number(*value))
        },
    );

    let process_eval_for_veto = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "veto!",
        "(veto!)",
        "Suppress the scheduler-owned base event for this step while allowing later processes to run.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("veto! expects no arguments".to_string());
            }
            let mut guard = process_eval_for_veto
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("veto! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "veto!")?;
            if ctx.step_context.is_none() {
                return Err("veto! requires a scheduler step event context".to_string());
            }
            ctx.commands
                .push(crate::process::ProcessRunCommand::VetoBaseEvent);
            Ok(EValue::Bool(true))
        },
    );

    let process_eval_for_step_length = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "step-length",
        "(step-length)",
        "Return the current scheduler grid step length in beats.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("step-length expects no arguments".to_string());
            }
            let guard = process_eval_for_step_length
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("step-length called outside process execution".to_string());
            };
            let Some(step_context) = ctx.step_context.as_ref() else {
                return Err("step-length requires a scheduler step event context".to_string());
            };
            Ok(EValue::Number(step_context.step_beats as f64))
        },
    );

    runtime.register_native_with_docs(
        "timebase-beats",
        "(timebase-beats index-or-name)",
        "Convert a sequencer timebase index/name to quarter-note beats.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("timebase-beats expects one timebase".to_string());
            }
            let timebase = match &args[0] {
                EValue::Number(index) => {
                    if !index.is_finite()
                        || *index < 0.0
                        || index.fract() != 0.0
                        || (*index as usize) >= Timebase::COUNT
                    {
                        return Err(format!(
                            "timebase-beats index must be an integer from 0 to {}",
                            Timebase::COUNT - 1
                        ));
                    }
                    Timebase::from_index(*index as u32)
                }
                value => parse_timebase_arg(std::slice::from_ref(value), 0)?,
            };
            Ok(EValue::Number(timebase.step_beats(16)))
        },
    );

    // The UI runtime already owns `track` as a widget/SDF combinator. Process
    // bodies execute in the scheduler scratch VM, where the name is free; do
    // not replace an established host meaning while registering authoring
    // natives in the UI VM.
    if runtime.global_value("track").is_none() {
        runtime.register_native_with_docs(
            "track",
            "(track index :param [:steps-ago n | :trigs-ago n])",
            "Construct a previous-tick resolved track-parameter read source.",
            move |args, _ctx| {
                if args.len() != 2 && args.len() != 4 {
                    return Err(
                        "track expects index, param, and optional :steps-ago/:trigs-ago count"
                            .to_string(),
                    );
                }
                let track = process_number_arg(args.first(), "track")?;
                if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
                    return Err("track index must be a non-negative integer".to_string());
                }
                let param = process_symbol_name(&args[1])?;
                let mut fields = vec![
                    ("kind", EValue::Keyword("track-read".to_string())),
                    ("track", EValue::Number(track)),
                    ("param", EValue::Keyword(param)),
                ];
                if args.len() == 4 {
                    let mode = process_symbol_name(&args[2])?;
                    if mode != "steps-ago" && mode != "trigs-ago" {
                        return Err(
                            "track read history mode must be :steps-ago or :trigs-ago".to_string()
                        );
                    }
                    let ago = process_number_arg(args.get(3), "track")?;
                    if !ago.is_finite() || ago < 0.0 || ago.fract() != 0.0 {
                        return Err(
                            "track read history count must be a non-negative integer".to_string()
                        );
                    }
                    if ago as usize >= crate::process::PROCESS_READ_HISTORY_DEPTH {
                        return Err(format!(
                            "track read history count must be less than {}",
                            crate::process::PROCESS_READ_HISTORY_DEPTH
                        ));
                    }
                    fields.push(("mode", EValue::Keyword(mode)));
                    fields.push(("ago", EValue::Number(ago)));
                }
                Ok(process_map(fields))
            },
        );
    }

    runtime.register_native_with_docs(
        "pitch-field",
        "(pitch-field pitches [:root pitch] [:weight 0..1])",
        "Construct a typed pitch-set suggestion field.",
        move |args, _ctx| {
            let Some(EValue::List(pitches)) = args.first() else {
                return Err("pitch-field expects a non-empty pitch list".to_string());
            };
            if pitches.is_empty() {
                return Err("pitch-field expects a non-empty pitch list".to_string());
            }
            let mut pitch_values = Vec::with_capacity(pitches.len());
            for pitch in pitches {
                let pitch = pitch.borrow();
                let pitch = process_number_arg(Some(&pitch), "pitch-field")?;
                if !pitch.is_finite() {
                    return Err("pitch-field pitches must be finite".to_string());
                }
                pitch_values.push(EValue::Number(pitch));
            }
            if (args.len() - 1) % 2 != 0 {
                return Err("pitch-field options must be keyword/value pairs".to_string());
            }
            let mut root = EValue::Nil;
            let mut weight = 1.0;
            let mut index = 1;
            while index < args.len() {
                match process_symbol_name(&args[index])?.as_str() {
                    "root" => {
                        let value = process_number_arg(args.get(index + 1), "pitch-field :root")?;
                        if !value.is_finite() {
                            return Err("pitch-field root must be finite".to_string());
                        }
                        root = EValue::Number(value);
                    }
                    "weight" => {
                        weight = process_number_arg(args.get(index + 1), "pitch-field :weight")?;
                        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
                            return Err("pitch-field weight must be between 0 and 1".to_string());
                        }
                    }
                    option => return Err(format!("pitch-field unknown option :{option}")),
                }
                index += 2;
            }
            Ok(process_map([
                ("field-domain", EValue::Keyword("pitch-field".to_string())),
                ("pitches", process_list(pitch_values)),
                ("root", root),
                ("weight", EValue::Number(weight)),
            ]))
        },
    );

    runtime.register_native_with_docs(
        "scalar-field",
        "(scalar-field value)",
        "Construct a typed scalar suggestion field.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("scalar-field expects one number".to_string());
            }
            process_scalar_field(process_number_arg(args.first(), "scalar-field")?)
        },
    );

    runtime.register_native_with_docs(
        "gate-field",
        "(gate-field value)",
        "Construct a typed gate suggestion field.",
        move |args, _ctx| match args.as_slice() {
            [EValue::Bool(value)] => Ok(process_gate_field(*value)),
            [EValue::Number(value)] => Ok(process_gate_field(*value > 0.5)),
            _ => Err("gate-field expects one boolean or number".to_string()),
        },
    );

    let process_eval_for_suggest = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "suggest",
        "(suggest :field value)",
        "Publish a typed field into the named channel at this process tick.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("suggest expects a field name and value".to_string());
            }
            let name = process_symbol_name(&args[0])?;
            let value = normalize_process_field(&args[1])?;
            let mut guard = process_eval_for_suggest
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("suggest called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "suggest")?;
            ctx.outputs.push(crate::process::ProcessOutput {
                name: format!("__field:{name}"),
                value: value.clone(),
            });
            Ok(value)
        },
    );

    let process_eval_for_hear = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "hear",
        "(hear :field)",
        "Read the newest typed field published strictly before this process tick.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("hear expects one field name".to_string());
            }
            let name = process_symbol_name(&args[0])?;
            let guard = process_eval_for_hear
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("hear called outside process execution".to_string());
            };
            Ok(ctx.reads.fields.get(&name).cloned().unwrap_or(EValue::Nil))
        },
    );

    runtime.register_native_with_docs(
        "field-domain",
        "(field-domain field)",
        "Return a typed field's domain keyword, or nil for no field.",
        move |args, _ctx| match args.as_slice() {
            [EValue::Nil] => Ok(EValue::Nil),
            [field] => Ok(EValue::Keyword(process_field_domain(field)?)),
            _ => Err("field-domain expects one field".to_string()),
        },
    );

    for (native, key) in [
        ("field-value", "value"),
        ("field-pitches", "pitches"),
        ("field-root", "root"),
        ("field-weight", "weight"),
    ] {
        runtime.register_native_with_docs(
            native,
            format!("({native} field)"),
            "Read a typed field component.",
            move |args, _ctx| match args.as_slice() {
                [EValue::Nil] => Ok(EValue::Nil),
                [field] => process_field_cell(field, key),
                _ => Err(format!("{native} expects one field")),
            },
        );
    }

    runtime.register_native_with_docs(
        "field-nearest-delta",
        "(field-nearest-delta pitch-field current-pitch grace)",
        "Return the shortest signed pitch-class delta toward a pitch field.",
        move |args, _ctx| {
            if args.len() != 3 || process_field_domain(&args[0])? != "pitch-field" {
                return Err(
                    "field-nearest-delta expects a pitch field, current pitch, and grace"
                        .to_string(),
                );
            }
            let current = process_number_arg(args.get(1), "field-nearest-delta")?;
            let grace = process_number_arg(args.get(2), "field-nearest-delta")?.max(0.0);
            let EValue::List(pitches) = process_field_cell(&args[0], "pitches")? else {
                return Err("pitch field pitches must be a list".to_string());
            };
            let mut best: Option<f64> = None;
            for pitch in pitches {
                let pitch = pitch.borrow();
                let pitch = process_number_arg(Some(&pitch), "field-nearest-delta")?;
                let delta = (pitch - current + 6.0).rem_euclid(12.0) - 6.0;
                if best.is_none_or(|best| delta.abs() < best.abs()) {
                    best = Some(delta);
                }
            }
            let delta = best.ok_or_else(|| "pitch field cannot be empty".to_string())?;
            Ok(EValue::Number(if delta.abs() <= grace {
                0.0
            } else {
                delta
            }))
        },
    );

    let process_eval_for_current_note = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "current-note",
        "(current-note)",
        "Return the current step event's resolved transpose before this process runs.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("current-note expects no arguments".to_string());
            }
            let guard = process_eval_for_current_note
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(step) = guard.as_ref().and_then(|ctx| ctx.step_context.as_ref()) else {
                return Err("current-note requires a scheduler step event context".to_string());
            };
            Ok(EValue::Number(step.resolved.transpose as f64))
        },
    );

    for (native, observe) in [("observed-tracks", true), ("play-tracks", false)] {
        let process_eval_for_tracks = Arc::clone(&process_eval);
        runtime.register_native_with_docs(
            native,
            format!("({native})"),
            "Return this conductor attachment's bound track indices.",
            move |args, _ctx| {
                if !args.is_empty() {
                    return Err(format!("{native} expects no arguments"));
                }
                let guard = process_eval_for_tracks
                    .lock()
                    .map_err(|_| "failed to lock process eval context".to_string())?;
                let Some(ctx) = guard.as_ref() else {
                    return Err(format!("{native} called outside process execution"));
                };
                let tracks = if observe {
                    &ctx.conductor_observe_tracks
                } else {
                    &ctx.conductor_play_tracks
                };
                if tracks.is_empty() {
                    return Err(format!("{native} requires a conductor attachment"));
                }
                Ok(process_list(
                    tracks.iter().map(|track| EValue::Number(*track as f64)),
                ))
            },
        );
    }

    let process_authoring_for_read_source = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "process",
        "(process name :state-or-outlet)",
        "Construct a process state/outlet read source.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("process expects a name and state/outlet field".to_string());
            }
            let process = match &args[0] {
                EValue::HostHandle { kind, id, .. } if kind == "process" => {
                    let registry = process_authoring_for_read_source
                        .lock()
                        .map_err(|_| "failed to lock process authoring registry".to_string())?;
                    let instance = registry
                        .instances
                        .iter()
                        .find(|instance| instance.handle_id.0 == *id)
                        .ok_or_else(|| "unknown process handle in read source".to_string())?;
                    instance
                        .name
                        .clone()
                        .unwrap_or_else(|| instance.class_name.clone())
                }
                value => process_symbol_name(value)?,
            };
            Ok(process_map([
                ("kind", EValue::Keyword("process-read".to_string())),
                ("process", EValue::String(process)),
                ("field", EValue::String(process_symbol_name(&args[1])?)),
            ]))
        },
    );

    let process_eval_for_read = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "read",
        "(read (track ...)) | (read (process ...)) | (read :channel :name)",
        "Read scheduler-owned resolved track history, process state/outlets, or a channel.",
        move |args, _ctx| {
            let guard = process_eval_for_read
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("read called outside process execution".to_string());
            };
            if args.len() == 2 && process_symbol_name(&args[0]).ok().as_deref() == Some("channel") {
                let name = process_symbol_name(&args[1])?;
                return Ok(ctx
                    .reads
                    .channels
                    .get(&name)
                    .cloned()
                    .unwrap_or(EValue::Nil));
            }
            let [EValue::Map(source)] = args.as_slice() else {
                return Err("read expects a track/process source or channel name".to_string());
            };
            let string_field = |name: &str| -> Result<String, String> {
                let value = source
                    .get(name)
                    .ok_or_else(|| format!("read source missing {name}"))?
                    .borrow();
                process_symbol_name(&value)
            };
            match string_field("kind")?.as_str() {
                "track-read" => {
                    let track = match source.get("track").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")? as usize,
                        None => return Err("track read source missing track".to_string()),
                    };
                    let param =
                        parse_step_param_arg(&[EValue::Keyword(string_field("param")?)], 0)?;
                    let Some(track) = ctx.reads.tracks.get(track) else {
                        return Ok(EValue::Number(param.default_value() as f64));
                    };
                    let mode = source
                        .get("mode")
                        .map(|_| string_field("mode"))
                        .transpose()?;
                    let ago = match source.get("ago").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")? as usize,
                        None => 0,
                    };
                    let values = match mode.as_deref() {
                        Some("steps-ago") => track.steps.get(ago).unwrap_or(&track.current),
                        Some("trigs-ago") => track.trigs.get(ago).unwrap_or(&track.current),
                        None => &track.current,
                        Some(_) => return Err("unknown track read history mode".to_string()),
                    };
                    Ok(EValue::Number(values[param.index()] as f64))
                }
                "process-read" => {
                    let process = string_field("process")?;
                    let field = string_field("field")?;
                    Ok(ctx
                        .reads
                        .process_values
                        .get(&process)
                        .and_then(|values| values.get(&field))
                        .cloned()
                        .unwrap_or(EValue::Nil))
                }
                _ => Err("unknown read source".to_string()),
            }
        },
    );

    let process_eval_for_ratchet = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "ratchet!",
        "(ratchet! :times n :mode :subdivide|:repeat :span beats :shape fn)",
        "Clone the current scheduler-owned base event into a ratchet burst.",
        move |args, _ctx| {
            let (times, mode, span_beats, shape) = parse_process_ratchet_args(&args)?;
            let mut guard = process_eval_for_ratchet
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("ratchet! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "ratchet!")?;
            let Some(step_context) = ctx.step_context.clone() else {
                return Err("ratchet! requires a scheduler step event context".to_string());
            };
            if times == 0 {
                return Ok(EValue::Bool(false));
            }
            ctx.commands
                .push(crate::process::ProcessRunCommand::Ratchet(
                    crate::process::ProcessRatchetRequest {
                        times,
                        mode,
                        span_beats,
                        shape,
                        shape_context: crate::process::ProcessRatchetShapeContext {
                            runtime_id: ctx.runtime_id,
                            beat: ctx.beat,
                            inlets: ctx.inlets.clone(),
                            state: ctx.state.clone(),
                            event: ctx.event.clone(),
                            step_context,
                            ports: ctx.ports.clone(),
                            random_state: ctx.random_state,
                        },
                    },
                ));
            Ok(EValue::Number(times as f64))
        },
    );

    register_process_ratchet_event_natives(runtime);

    let process_eval_for_target_add = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "target-add!",
        "(target-add! value) | (target-add! :port value)",
        "Add a value to one of this process run's typed target ports.",
        move |args, _ctx| {
            let (port, value) = process_target_write_args(&args, "target-add!")?;
            push_process_target_write(
                &process_eval_for_target_add,
                crate::process::ProcessTargetOp::Add,
                port,
                value,
            )?;
            Ok(EValue::Number(value as f64))
        },
    );

    let process_eval_for_target_set = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "target-set!",
        "(target-set! value) | (target-set! :port value)",
        "Set one of this process run's typed target ports.",
        move |args, _ctx| {
            let (port, value) = process_target_write_args(&args, "target-set!")?;
            push_process_target_write(
                &process_eval_for_target_set,
                crate::process::ProcessTargetOp::Set,
                port,
                value,
            )?;
            Ok(EValue::Number(value as f64))
        },
    );

    let process_eval_for_rand = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "rand",
        "(rand)",
        "Deterministic process-scoped pseudo-random float in [0,1).",
        move |_args, _ctx| {
            let mut guard = process_eval_for_rand
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("rand called outside process execution".to_string());
            };
            ctx.random_state = ctx.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let bits = gen_splitmix64(ctx.random_state);
            let unit = ((bits >> 11) as f64) * (1.0 / ((1u64 << 53) as f64));
            Ok(EValue::Number(unit))
        },
    );

    runtime.register_native_with_docs(
        "clip",
        "(clip value low high)",
        "Clamp value to the inclusive numeric range.",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "clip")?;
            let low = process_number_arg(args.get(1), "clip")?;
            let high = process_number_arg(args.get(2), "clip")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || low > high {
                return Ok(EValue::Number(f64::NAN));
            }
            Ok(EValue::Number(value.clamp(low, high)))
        },
    );

    runtime.register_native_with_docs(
        "wrap",
        "(wrap value low high)",
        "Wrap value into the half-open numeric range [low, high).",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "wrap")?;
            let low = process_number_arg(args.get(1), "wrap")?;
            let high = process_number_arg(args.get(2), "wrap")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || high <= low {
                return Ok(EValue::Number(f64::NAN));
            }
            let span = high - low;
            let mut wrapped = (value - low) % span;
            if wrapped < 0.0 {
                wrapped += span;
            }
            Ok(EValue::Number(low + wrapped))
        },
    );

    runtime.register_native_with_docs(
        "bounce",
        "(bounce value low high)",
        "Fold value into a ping-pong numeric range.",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "bounce")?;
            let low = process_number_arg(args.get(1), "bounce")?;
            let high = process_number_arg(args.get(2), "bounce")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || high <= low {
                return Ok(EValue::Number(f64::NAN));
            }
            let span = high - low;
            let period = span * 2.0;
            let mut phase = (value - low) % period;
            if phase < 0.0 {
                phase += period;
            }
            let folded = if phase <= span {
                low + phase
            } else {
                high - (phase - span)
            };
            Ok(EValue::Number(folded))
        },
    );

    runtime.register_native_with_docs(
        "gate?",
        "(gate? value)",
        "Return true when a gate-like value is active.",
        move |args, _ctx| {
            Ok(EValue::Bool(match args.first() {
                Some(EValue::Bool(value)) => *value,
                Some(EValue::Number(value)) => *value > 0.5,
                Some(EValue::Nil) | None => false,
                _ => true,
            }))
        },
    );

    let process_authoring_for_ps = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "ps",
        "(ps)",
        "Return authored process/channel status.",
        move |_args, _ctx| {
            let registry = process_authoring_for_ps
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            Ok(process_status_value(&registry))
        },
    );
}

fn register_def_accumulator_dispatch_native(
    runtime: &mut Runtime,
    accumulators: SharedRegisteredAccumulators,
    process_authoring: SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) {
    // A vm native (not a plain native) so the process-accumulator branch can
    // register the class constructor, matching def-process.
    runtime.register_vm_native_with_docs(
        "def-accumulator",
        "(def-accumulator name body) | (def-accumulator name :target (step-param :transpose) :amount (...))",
        "Define either a legacy script accumulator or a process accumulator, depending on the argument shape.",
        move |args, vm| {
            let result = def_accumulator_dispatch(
                args,
                vm,
                &accumulators,
                &process_authoring,
                process_chain_state.clone(),
                publish.clone(),
            );
            match result {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("[process] def-accumulator error: {error}");
                    EValue::Bool(false)
                }
            }
        },
    );
}

fn def_accumulator_dispatch(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    accumulators: &SharedRegisteredAccumulators,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let is_legacy_script_form = args.len() == 2 && !matches!(args.get(1), Some(EValue::Keyword(_)));
    if !is_legacy_script_form {
        return register_process_accumulator_def(
            args,
            vm,
            process_authoring,
            process_chain_state,
            publish,
        );
    }
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "expected accumulator name".to_string())?,
    )?;
    let callback = args
        .get(1)
        .ok_or_else(|| "expected accumulator callback".to_string())?;
    let callback = match callback {
        EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
        EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
        other => RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other)),
    };
    let mut registry = accumulators
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
    Ok(EValue::Bool(true))
}

fn register_process_graph_emit_native(
    runtime: &mut Runtime,
    process_eval: SharedProcessEvalContext,
) {
    runtime.register_native_with_docs(
        "emit",
        "(emit :track n :after beats :note n :vel v :duration d) or (emit :note n :vel v :dur d)",
        "Emit a process event when called from a process body; otherwise build a graph update emit map.",
        move |args, _ctx| {
            let mut guard = process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            if let Some(ctx) = guard.as_mut() {
                ensure_process_run_scope(ctx, "emit")?;
                let event = build_process_emit_event(&args)?;
                if !ctx.conductor_play_tracks.is_empty() {
                    let Some(track) = event.track else {
                        return Err("conductor emit requires an explicit bound :track".to_string());
                    };
                    if !ctx.conductor_play_tracks.contains(&track) {
                        return Err(format!(
                            "conductor cannot emit to unbound track {track}; bound play tracks are {:?}",
                            ctx.conductor_play_tracks
                        ));
                    }
                }
                ctx.emissions.push(event);
                return Ok(EValue::Bool(true));
            }
            drop(guard);
            graph_update::build_graph_emit_value(&args)
        },
    );
}

pub fn register_published_process_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    ui_epoch: Arc<AtomicUsize>,
) -> PublishedProcessAuthoringNatives {
    let process_authoring = Arc::new(Mutex::new(ProcessAuthoringRegistry::with_handle_base(
        UI_PROCESS_HANDLE_BASE,
    )));
    let process_eval = Arc::new(Mutex::new(None));
    let state_for_publish = Arc::clone(&state);
    let publish: ProcessPublishHook = Arc::new(move |snapshot| {
        state_for_publish.publish_process_authoring(snapshot);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
    });
    register_process_natives(
        runtime,
        Arc::clone(&process_authoring),
        process_eval,
        Some(Arc::clone(&publish)),
        Some(Arc::clone(&state)),
        false,
    );
    register_process_chain_natives(
        runtime,
        Arc::clone(&state),
        Arc::clone(&process_authoring),
        Some(Arc::clone(&publish)),
        true,
    );
    PublishedProcessAuthoringNatives {
        process_authoring,
        process_chain_state: state,
        publish: Some(publish),
    }
}

const PROCESS_LANE_TAG: &str = "__process-lane";
const PROCESS_INLET_INSTANCE_TARGET_TAG: &str = "__process-inlet-instance";

/// `(lane 0 1 0 ...)` evaluates to a tagged list; `processes`/`lane!` unpack it.
fn process_lane_values(value: &EValue) -> Option<Result<Vec<f32>, String>> {
    let EValue::List(items) = value else {
        return None;
    };
    match items.first().map(|item| item.borrow().clone()) {
        Some(EValue::Keyword(tag)) if tag.trim_start_matches(':') == PROCESS_LANE_TAG => {}
        _ => return None,
    }
    let mut values = Vec::with_capacity(items.len().saturating_sub(1));
    for item in &items[1..] {
        match &*item.borrow() {
            EValue::Number(number) => values.push(*number as f32),
            EValue::Bool(gate) => values.push(if *gate { 1.0 } else { 0.0 }),
            other => {
                return Some(Err(format!(
                    "lane values must be numbers, got {}",
                    eseqlisp::vm::format_lisp_value(other)
                )));
            }
        }
    }
    Some(Ok(values))
}

fn process_lane_literal(values: &[f32]) -> EValue {
    let mut items = vec![EValue::Keyword(PROCESS_LANE_TAG.to_string())];
    items.extend(values.iter().map(|value| EValue::Number(*value as f64)));
    process_list(items)
}

fn parse_process_track_spec(value: &EValue, active_tracks: usize) -> Result<Vec<usize>, String> {
    let parse_index = |number: f64| -> Result<usize, String> {
        if number < 0.0 || number.fract() != 0.0 {
            return Err("processes expects non-negative integer track indices".to_string());
        }
        let track = number as usize;
        if track >= active_tracks {
            return Err(format!("track {track} out of range"));
        }
        Ok(track)
    };
    match value {
        EValue::Number(number) => Ok(vec![parse_index(*number)?]),
        EValue::Keyword(name) | EValue::Symbol(name) if name.trim_start_matches(':') == "all" => {
            Err(
                "(processes :track :all) is deprecated: use (processes :project ...) for a \
                 project-wide layer every track (present and future) runs, or (list 0 1 ...) \
                 to stamp independent copies on a track set"
                    .to_string(),
            )
        }
        EValue::List(items) => {
            let mut tracks = Vec::with_capacity(items.len());
            for item in items {
                match &*item.borrow() {
                    EValue::Number(number) => tracks.push(parse_index(*number)?),
                    _ => return Err("processes :track list expects track indices".to_string()),
                }
            }
            Ok(tracks)
        }
        _ => Err("processes expects :track <index | (list ...)>".to_string()),
    }
}

/// Convert an attached instance handle into a pattern-scoped chain slot:
/// scalar inlet literals into `slot.inlets`, `(lane ...)` literals into
/// `slot.lanes` (legal only on `:lane true` inlets).
fn process_chain_slot_from_handle(
    registry: &ProcessAuthoringRegistry,
    handle_id: u64,
) -> Result<crate::process::TrackProcessSlot, String> {
    let instance = registry
        .instances
        .iter()
        .find(|entry| entry.handle_id.0 == handle_id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == instance.class_name)
        .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
    let mut inlets = std::collections::BTreeMap::new();
    let mut lanes = std::collections::BTreeMap::new();
    for (name, value) in &instance.inlets {
        let crate::process::ProcessInletValue::Literal(literal) = value else {
            return Err(format!(
                "chain-attached process inlet '{name}' must be a literal (patch outlets/channels with `patch` instead)"
            ));
        };
        if let Some(lane_values) = process_lane_values(literal) {
            let lane_backed = def
                .inlets
                .iter()
                .any(|inlet| inlet.name == *name && inlet.lane);
            if !lane_backed {
                return Err(format!(
                    "inlet '{name}' of '{}' is not lane-backed (:lane true)",
                    instance.class_name
                ));
            }
            lanes.insert(
                name.clone(),
                crate::process::ProcessLane {
                    values: lane_values?,
                },
            );
        } else {
            inlets.insert(
                name.clone(),
                crate::process::ProcessLiteral::from_value(literal)?,
            );
        }
    }
    validate_process_port_bindings(def, &instance.bindings)?;
    let mut bindings: BTreeMap<String, Option<crate::process::ParamTarget>> = def
        .ports
        .iter()
        .map(|port| (port.name.clone(), None))
        .collect();
    for (port, target) in &instance.bindings {
        bindings.insert(port.clone(), target.clone());
    }
    Ok(crate::process::TrackProcessSlot {
        instance_id: crate::process::ProcessInstanceId(handle_id),
        instance_name: instance.name.clone(),
        class_name: instance.class_name.clone(),
        enabled: true,
        project_layer: false,
        inlets,
        lanes,
        bindings,
    })
}

fn validate_process_port_bindings(
    def: &crate::process::ProcessDef,
    bindings: &BTreeMap<String, Option<crate::process::ParamTarget>>,
) -> Result<(), String> {
    for (port_name, target) in bindings {
        let Some(port) = def.ports.iter().find(|port| port.name == *port_name) else {
            return Err(format!(
                "process '{}' has no target port '{}'",
                def.name, port_name
            ));
        };
        if let Some(target) = target {
            if !port.allows_binding_target(target) {
                return Err(format!(
                    "target {} is incompatible with process '{}' port '{}'",
                    process_param_target_label_for_error(target),
                    def.name,
                    port_name
                ));
            }
        }
    }
    Ok(())
}

fn process_param_target_label_for_error(target: &crate::process::ParamTarget) -> String {
    match target {
        crate::process::ParamTarget::StepParam { param } => format!("step-param:{param}"),
        crate::process::ParamTarget::InstrumentParam { param, .. } => {
            format!("instrument:{param}")
        }
        crate::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => format!("effect{}:{effect}:{param}", slot + 1),
        crate::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            format!("midi-fx{}:{fx}:{param}", slot + 1)
        }
        crate::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } => instance_id
            .map(|id| format!("process-inlet:{process}#{}:{inlet}", id.0))
            .unwrap_or_else(|| format!("process-inlet:{process}:{inlet}")),
        crate::process::ParamTarget::RackSlotParam { slot, param } => {
            format!("rack{}:{param}", slot + 1)
        }
        crate::process::ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            format!("rack{}:instrument:{param}", slot + 1)
        }
        crate::process::ParamTarget::RackMacroParam { macro_id } => {
            format!("rack-macro:{}", macro_id + 1)
        }
    }
}

fn matching_existing_process_slot<'a>(
    slot: &crate::process::TrackProcessSlot,
    existing: &'a crate::process::TrackProcessChain,
) -> Option<&'a crate::process::TrackProcessSlot> {
    existing
        .slots
        .iter()
        .find(|existing_slot| process_slots_have_same_identity(slot, existing_slot))
}

fn process_slots_have_same_identity(
    left: &crate::process::TrackProcessSlot,
    right: &crate::process::TrackProcessSlot,
) -> bool {
    if let Some(name) = left.instance_name.as_deref() {
        return right.class_name == left.class_name && right.instance_name.as_deref() == Some(name);
    }
    right.instance_id == left.instance_id && right.class_name == left.class_name
}

fn preserve_process_slot_state(
    defs: &[crate::process::ProcessDef],
    existing: &crate::process::TrackProcessChain,
    replacement: &mut crate::process::TrackProcessChain,
) {
    // Once a chain exists, its order is pattern-owned UI state. Scratch
    // re-evaluation reconciles the attachment set without undoing drag reorder:
    // retained instances keep their existing relative order and newly authored
    // instances are appended in declaration order. An explicitly empty
    // `processes` form still clears the chain.
    let mut pending = std::mem::take(&mut replacement.slots);
    let mut ordered = Vec::with_capacity(pending.len());
    for existing_slot in &existing.slots {
        if let Some(index) = pending
            .iter()
            .position(|slot| process_slots_have_same_identity(slot, existing_slot))
        {
            ordered.push(pending.remove(index));
        }
    }
    ordered.extend(pending);
    replacement.slots = ordered;

    for slot in &mut replacement.slots {
        let Some(existing_slot) = matching_existing_process_slot(slot, existing) else {
            continue;
        };
        slot.enabled = existing_slot.enabled;
        let Some(def) = defs.iter().find(|def| def.name == slot.class_name) else {
            continue;
        };
        for inlet in &def.inlets {
            if inlet.lane {
                if let Some(lane) = existing_slot.lanes.get(&inlet.name) {
                    slot.lanes.insert(inlet.name.clone(), lane.clone());
                }
            }
            if slot.inlets.contains_key(&inlet.name) {
                if let Some(value) = existing_slot.inlets.get(&inlet.name) {
                    slot.inlets.insert(inlet.name.clone(), value.clone());
                }
            } else if inlet.lane {
                if let Some(value) = existing_slot.inlets.get(&inlet.name) {
                    slot.inlets.insert(inlet.name.clone(), value.clone());
                }
            }
        }
        for port in &def.ports {
            let Some(Some(target)) = existing_slot.bindings.get(&port.name) else {
                continue;
            };
            if port.allows_binding_target(target) && slot.bindings.contains_key(&port.name) {
                slot.bindings
                    .insert(port.name.clone(), Some(target.clone()));
            }
        }
    }
}

fn register_process_chain_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    process_authoring: SharedProcessAuthoring,
    publish: Option<ProcessPublishHook>,
    write_process_chain_state: bool,
) {
    runtime.register_native_with_docs(
        "lane",
        "(lane v1 v2 ...)",
        "Per-step lane literal for a lane-backed process inlet. Steps beyond the lane's length read the inlet default.",
        move |args, _ctx| {
            let mut items = vec![EValue::Keyword(PROCESS_LANE_TAG.to_string())];
            for arg in &args {
                match arg {
                    EValue::Number(_) | EValue::Bool(_) => items.push(arg.clone()),
                    other => {
                        return Err(format!(
                            "lane values must be numbers, got {}",
                            eseqlisp::vm::format_lisp_value(other)
                        ))
                    }
                }
            }
            Ok(process_list(items))
        },
    );

    let state_for_processes = Arc::clone(&state);
    let authoring_for_processes = Arc::clone(&process_authoring);
    let publish_for_processes = publish.clone();
    runtime.register_native_with_docs(
        "processes",
        "(processes :track tracks instance...) | (processes :project instance...) | (processes :observe tracks :play tracks instance)",
        "Declare a track/project step chain or one conductor instance observing and playing track sets.",
        move |args, _ctx| {
            let (EValue::Keyword(key) | EValue::Symbol(key)) = args
                .first()
                .ok_or_else(|| "processes expects :track, :project, or :observe first".to_string())?
            else {
                return Err("processes expects :track, :project, or :observe first".to_string());
            };
            if key.trim_start_matches(':') == "observe" {
                let observe_tracks = parse_process_track_spec(
                    args.get(1)
                        .ok_or_else(|| "processes :observe expects a track list".to_string())?,
                    state_for_processes.active_track_count(),
                )?;
                if observe_tracks.is_empty() {
                    return Err("processes :observe requires at least one track".to_string());
                }
                if observe_tracks.iter().copied().collect::<HashSet<_>>().len()
                    != observe_tracks.len()
                {
                    return Err("processes :observe track list contains duplicates".to_string());
                }
                if process_symbol_name(
                    args.get(2)
                        .ok_or_else(|| "processes :observe expects :play".to_string())?,
                )? != "play"
                {
                    return Err("processes :observe expects :play after observed tracks".to_string());
                }
                let play_tracks = parse_process_track_spec(
                    args.get(3)
                        .ok_or_else(|| "processes :play expects a track list".to_string())?,
                    state_for_processes.active_track_count(),
                )?;
                if play_tracks.is_empty() {
                    return Err("processes :play requires at least one track".to_string());
                }
                if play_tracks.iter().copied().collect::<HashSet<_>>().len() != play_tracks.len() {
                    return Err("processes :play track list contains duplicates".to_string());
                }
                let [EValue::HostHandle { kind, id, .. }] = args.get(4..).unwrap_or(&[]) else {
                    return Err(
                        "conductor attachment expects exactly one process instance".to_string()
                    );
                };
                if kind != "process" {
                    return Err("conductor attachment expects a process instance".to_string());
                }
                {
                    let mut registry = authoring_for_processes
                        .lock()
                        .map_err(|_| "failed to lock process registry".to_string())?;
                    if !registry.instances.iter().any(|instance| instance.handle_id.0 == *id) {
                        return Err("unknown conductor process handle".to_string());
                    }
                    registry
                        .conductors
                        .retain(|entry| entry.process_handle_id.0 != *id);
                    registry
                        .conductors
                        .push(crate::process::AuthoredConductorAttachment {
                            process_handle_id: crate::process::AuthoredHandleId(*id),
                            observe_tracks,
                            play_tracks,
                        });
                }
                publish_process_authoring(&authoring_for_processes, &publish_for_processes);
                return Ok(args[4].clone());
            }
            let (tracks, instance_args) = match key.trim_start_matches(':') {
                "track" => {
                    let tracks = parse_process_track_spec(
                        args.get(1).ok_or_else(|| {
                            "processes expects a track spec after :track".to_string()
                        })?,
                        state_for_processes.active_track_count(),
                    )?;
                    (Some(tracks), args.get(2..).unwrap_or(&[]))
                }
                "project" => (None, args.get(1..).unwrap_or(&[])),
                _ => {
                    return Err(
                        "processes expects :track, :project, or :observe first".to_string()
                    )
                }
            };
            let project_layer = tracks.is_none();
            let mut slots = Vec::new();
            let mut handles = Vec::new();
            let defs = {
                let registry = authoring_for_processes
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                for arg in instance_args {
                    let EValue::HostHandle { kind, id, .. } = arg else {
                        return Err(
                            "processes expects process instances, e.g. (transpose-climb :limit 12)"
                                .to_string(),
                        );
                    };
                    if kind != "process" {
                        return Err(format!("processes expects process instances, got {kind}"));
                    }
                    if slots.iter().any(
                        |slot: &crate::process::TrackProcessSlot| slot.instance_id.0 == *id,
                    ) {
                        return Err(
                            "the same process instance cannot appear twice in one chain; construct a second instance instead"
                                .to_string(),
                        );
                    }
                    let mut slot = process_chain_slot_from_handle(&registry, *id)?;
                    slot.project_layer = project_layer;
                    slots.push(slot);
                    handles.push(arg.clone());
                }
                registry.defs.clone()
            };
            let chain = crate::process::TrackProcessChain { slots };
            if write_process_chain_state {
                match &tracks {
                    Some(tracks) => {
                        for track in tracks {
                            let mut track_chain = chain.clone();
                            if let Some(existing) = state_for_processes.track_process_chain(*track)
                            {
                                preserve_process_slot_state(&defs, &existing, &mut track_chain);
                            }
                            if !state_for_processes.set_track_process_chain(*track, track_chain) {
                                return Err(format!("track {track} out of range"));
                            }
                        }
                    }
                    None => {
                        let mut project_chain = chain.clone();
                        let existing = state_for_processes.project_process_chain();
                        preserve_process_slot_state(&defs, &existing, &mut project_chain);
                        if !state_for_processes.set_project_process_chain(project_chain) {
                            return Err("failed to update the project process layer".to_string());
                        }
                    }
                }
                publish_process_authoring(&authoring_for_processes, &publish_for_processes);
            }
            match handles.len() {
                0 => Ok(EValue::Bool(true)),
                1 => Ok(handles.remove(0)),
                _ => Ok(process_list(handles)),
            }
        },
    );

    let state_for_lane = Arc::clone(&state);
    let authoring_for_lane = Arc::clone(&process_authoring);
    let publish_for_lane = publish.clone();
    runtime.register_native_with_docs(
        "lane!",
        "(lane! instance :inlet v1 v2 ...)",
        "Replace a lane on an attached process instance in the current pattern (every track it is attached to).",
        move |args, _ctx| {
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("lane! expects a process instance handle".to_string());
            };
            if kind != "process" {
                return Err("lane! expects a process instance handle".to_string());
            }
            let inlet = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "lane! expects an :inlet name".to_string())?,
            )?;
            let mut values = Vec::with_capacity(args.len().saturating_sub(2));
            for arg in &args[2..] {
                match arg {
                    EValue::Number(number) => values.push(*number as f32),
                    EValue::Bool(gate) => values.push(if *gate { 1.0 } else { 0.0 }),
                    other => {
                        return Err(format!(
                            "lane! values must be numbers, got {}",
                            eseqlisp::vm::format_lisp_value(other)
                        ))
                    }
                }
            }
            {
                let registry = authoring_for_lane
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                let lane_backed = registry
                    .defs
                    .iter()
                    .find(|def| def.name == instance.class_name)
                    .map(|def| {
                        def.inlets
                            .iter()
                            .any(|entry| entry.name == inlet && entry.lane)
                    })
                    .unwrap_or(false);
                if !lane_backed {
                    return Err(format!(
                        "inlet '{inlet}' of '{}' is not lane-backed (:lane true)",
                        instance.class_name
                    ));
                }
            }
            let updated = if write_process_chain_state {
                let updated = state_for_lane.set_process_lane_values(
                    crate::process::ProcessInstanceId(*id),
                    &inlet,
                    values.clone(),
                );
                if updated == 0 {
                    return Err(
                        "process instance is not attached to any track (use `processes` first)"
                            .to_string(),
                    );
                }
                updated
            } else {
                0
            };
            {
                let mut registry = authoring_for_lane
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter_mut()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                instance.inlets.insert(
                    inlet.clone(),
                    crate::process::ProcessInletValue::Literal(process_lane_literal(&values)),
                );
            }
            if write_process_chain_state {
                publish_process_authoring(&authoring_for_lane, &publish_for_lane);
            }
            Ok(EValue::Number(updated as f64))
        },
    );

    let state_for_connect = Arc::clone(&state);
    let authoring_for_connect = Arc::clone(&process_authoring);
    let publish_for_connect = publish;
    runtime.register_native_with_docs(
        "connect!",
        "(connect! instance :port (inlet target-instance :inlet))",
        "Connect a process output port to another process instance's inlet.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err("connect! expects a process handle, port, and inlet target".to_string());
            }
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("connect! expects a process handle".to_string());
            };
            if kind != "process" {
                return Err("connect! expects a process handle".to_string());
            }
            let port = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "connect! expects a port name".to_string())?,
            )?;
            let target = parse_process_connection_target(
                args.get(2)
                    .ok_or_else(|| "connect! expects an inlet target".to_string())?,
            )?;
            {
                let mut registry = authoring_for_connect
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                let def = registry
                    .defs
                    .iter()
                    .find(|def| def.name == instance.class_name)
                    .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
                let single = BTreeMap::from([(port.clone(), Some(target.clone()))]);
                validate_process_port_bindings(def, &single)?;
                let instance = registry
                    .instances
                    .iter_mut()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                instance.bindings.insert(port.clone(), Some(target.clone()));
            }
            let updated = if write_process_chain_state {
                state_for_connect.set_process_port_binding_for_instance(
                    crate::process::ProcessInstanceId(*id),
                    &port,
                    target,
                )
            } else {
                0
            };
            if write_process_chain_state {
                publish_process_authoring(&authoring_for_connect, &publish_for_connect);
            }
            Ok(EValue::Number(updated as f64))
        },
    );
}

fn register_process_def(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "def-process expects a process name".to_string())?,
    )?;
    let mut def = parse_process_def(&name, &args[1..])?;
    def.source_path = vm
        .current_source_module()
        .map(|path| path.to_string_lossy().into_owned());
    process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?
        .upsert_def(def.clone());
    publish_process_authoring(process_authoring, &publish);
    register_process_constructor_native(vm, &name, process_authoring, process_chain_state, publish);
    Ok(EValue::String(name))
}

fn register_process_constructor_native(
    vm: &mut eseqlisp::vm::VM,
    name: &str,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) {
    let process_authoring_for_constructor = Arc::clone(process_authoring);
    let chain_state_for_constructor = process_chain_state;
    let publish_for_constructor = publish;
    let class_name = name.to_string();
    vm.register_native_with_vm(name, move |ctor_args, vm| {
        let inlet_names = ctor_args
            .iter()
            .filter_map(|arg| match arg {
                EValue::Keyword(inlet) => Some(inlet.trim_start_matches(':').to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        match construct_process_instance(
            &process_authoring_for_constructor,
            &class_name,
            ctor_args,
            false,
            false,
            None,
            None,
            false,
            chain_state_for_constructor.clone(),
            publish_for_constructor.clone(),
        ) {
            Ok(value) => {
                for inlet in inlet_names {
                    vm.attach_inline_widget_runtime_target(&class_name, &inlet, value.clone());
                }
                publish_process_authoring(
                    &process_authoring_for_constructor,
                    &publish_for_constructor,
                );
                value
            }
            Err(error) => {
                eprintln!("[process] constructor error: {error}");
                EValue::Bool(false)
            }
        }
    });
}

fn register_process_accumulator_def(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "def-accumulator expects a process name".to_string())?,
    )?;
    let mut def = parse_process_accumulator_def(&name, &args[1..])?;
    def.source_path = vm
        .current_source_module()
        .map(|path| path.to_string_lossy().into_owned());
    process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?
        .upsert_def(def);
    publish_process_authoring(process_authoring, &publish);
    register_process_constructor_native(vm, &name, process_authoring, process_chain_state, publish);
    Ok(EValue::String(name))
}

fn parse_process_accumulator_def(
    name: &str,
    args: &[EValue],
) -> Result<crate::process::ProcessDef, String> {
    let mut target_port = None;
    let mut target_kind = None;
    let mut target_hint = None;
    let mut amount = None;
    let mut reset_lane = false;
    let mut range = None;
    let mut mode = crate::process::ProcessAccumulatorMode::Wrap;
    let mut seed_policy = crate::process::ProcessSeedPolicy::Locked;
    let mut doc = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-accumulator missing value for :{key}"));
        };
        match key.as_str() {
            "target" => {
                if target_port.is_some() {
                    return Err("def-accumulator cannot specify :target more than once".to_string());
                }
                target_port = Some(parse_process_accumulator_target(value)?);
            }
            "target-kind" => target_kind = Some(parse_process_target_kind(value)?),
            "target-hint" => target_hint = Some(parse_process_target_hint(value)?),
            "amount" => amount = Some(parse_process_accumulator_amount_inlet(value)?),
            "reset" => {
                let reset = process_symbol_name(value)?.to_ascii_lowercase();
                reset_lane = reset == "lane";
                if !reset_lane && reset != "none" {
                    return Err("def-accumulator :reset supports :lane or :none".to_string());
                }
            }
            "range" => range = Some(parse_process_accumulator_range(value)?),
            "mode" => mode = parse_process_accumulator_mode(value)?,
            "seed" => seed_policy = parse_process_seed_policy(value)?,
            "doc" => {
                if let EValue::String(value) = value {
                    doc = Some(value.clone());
                }
            }
            other => return Err(format!("def-accumulator unknown key :{other}")),
        }
        idx += 1;
    }
    let mut target_port =
        target_port.ok_or_else(|| "def-accumulator requires :target".to_string())?;
    if target_port.is_mappable() {
        if let Some(kind) = target_kind {
            if kind == crate::process::ProcessTargetKind::ProcessInlet {
                return Err(
                    "def-accumulator does not expose connectable ports; use def-process with :targets ((out :process-inlet))"
                        .to_string(),
                );
            }
            target_port.target_kind = Some(kind);
        }
        if let Some(hint) = target_hint {
            if let Some(kind) = target_port.target_kind {
                if !kind.matches_hint(&hint) {
                    return Err(format!(
                        "def-accumulator :target-hint kind {} is incompatible with :target-kind {}",
                        hint.target_kind().as_str(),
                        kind.as_str()
                    ));
                }
            }
            target_port.target = Some(hint);
        }
    } else if target_kind.is_some() || target_hint.is_some() {
        return Err(
            "def-accumulator :target-kind and :target-hint require :target :mappable".to_string(),
        );
    }
    let amount = amount.ok_or_else(|| "def-accumulator requires :amount".to_string())?;
    let mut inlets = vec![amount.clone()];
    let reset_inlet = if reset_lane {
        let inlet = crate::process::ProcessInletDef {
            name: "reset".to_string(),
            kind: crate::process::ProcessInletKind::Gate,
            min: Some(0.0),
            max: Some(1.0),
            default: EValue::Number(0.0),
            lane: true,
            doc: None,
        };
        inlets.push(inlet);
        Some("reset".to_string())
    } else {
        None
    };
    Ok(crate::process::ProcessDef {
        id: crate::process::stable_process_id(name),
        name: name.to_string(),
        source_path: None,
        doc,
        inlets,
        outlets: Vec::new(),
        state: Vec::new(),
        every: None,
        seed_policy,
        ports: vec![target_port],
        accumulator: Some(crate::process::ProcessAccumulatorSpec {
            amount_inlet: amount.name,
            reset_inlet,
            range,
            mode,
        }),
        run_source: None,
        listens: Vec::new(),
    })
}

fn process_symbol_name(value: &EValue) -> Result<String, String> {
    match value {
        EValue::String(name) | EValue::Symbol(name) | EValue::Keyword(name) => Ok(name
            .trim_start_matches(':')
            .trim_start_matches('@')
            .to_string()),
        _ => Err("expected symbol/string name".to_string()),
    }
}

fn process_key_arg(value: Option<&EValue>, native: &str) -> Result<String, String> {
    process_symbol_name(value.ok_or_else(|| format!("{native} expects a key"))?)
}

fn process_number_arg(value: Option<&EValue>, native: &str) -> Result<f64, String> {
    match value {
        Some(EValue::Number(value)) => Ok(*value),
        _ => Err(format!("{native} expects a number")),
    }
}

fn process_field_domain(value: &EValue) -> Result<String, String> {
    let EValue::Map(map) = value else {
        return Err("expected a typed field value".to_string());
    };
    let domain = map
        .get("field-domain")
        .ok_or_else(|| "field value is missing field-domain".to_string())?
        .borrow();
    process_symbol_name(&domain)
}

fn process_field_cell(value: &EValue, key: &str) -> Result<EValue, String> {
    let EValue::Map(map) = value else {
        return Err("expected a typed field value".to_string());
    };
    map.get(key)
        .map(|value| value.borrow().clone())
        .ok_or_else(|| format!("field value is missing {key}"))
}

fn process_scalar_field(value: f64) -> Result<EValue, String> {
    if !value.is_finite() {
        return Err("scalar field value must be finite".to_string());
    }
    Ok(process_map([
        ("field-domain", EValue::Keyword("scalar".to_string())),
        ("value", EValue::Number(value)),
    ]))
}

fn process_gate_field(value: bool) -> EValue {
    process_map([
        ("field-domain", EValue::Keyword("gate".to_string())),
        ("value", EValue::Bool(value)),
    ])
}

fn normalize_process_field(value: &EValue) -> Result<EValue, String> {
    match value {
        EValue::Number(value) => process_scalar_field(*value),
        EValue::Bool(value) => Ok(process_gate_field(*value)),
        EValue::Map(_) => match process_field_domain(value)?.as_str() {
            "scalar" => {
                let scalar =
                    process_number_arg(Some(&process_field_cell(value, "value")?), "scalar field")?;
                process_scalar_field(scalar)
            }
            "gate" => match process_field_cell(value, "value")? {
                EValue::Bool(value) => Ok(process_gate_field(value)),
                _ => Err("gate field value must be boolean".to_string()),
            },
            "pitch-field" => {
                let pitches = process_field_cell(value, "pitches")?;
                let EValue::List(items) = &pitches else {
                    return Err("pitch-field pitches must be a list".to_string());
                };
                if items.is_empty() {
                    return Err("pitch-field requires at least one pitch".to_string());
                }
                for pitch in items {
                    let pitch = pitch.borrow();
                    let pitch = process_number_arg(Some(&pitch), "pitch-field")?;
                    if !pitch.is_finite() {
                        return Err("pitch-field pitches must be finite".to_string());
                    }
                }
                let weight = process_number_arg(
                    Some(&process_field_cell(value, "weight")?),
                    "pitch-field weight",
                )?;
                if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
                    return Err("pitch-field weight must be between 0 and 1".to_string());
                }
                Ok(value.clone())
            }
            domain => Err(format!("unknown field domain :{domain}")),
        },
        _ => Err("suggest expects a number, boolean, or typed field value".to_string()),
    }
}

fn process_target_write_args(
    args: &[EValue],
    native: &str,
) -> Result<(Option<String>, f32), String> {
    match args {
        [EValue::Number(value)] => Ok((None, *value as f32)),
        [port, EValue::Number(value)] => Ok((Some(process_symbol_name(port)?), *value as f32)),
        _ => Err(format!("{native} expects (value) or (:port value)")),
    }
}

fn ensure_process_run_scope(ctx: &ProcessEvalContext, native: &str) -> Result<(), String> {
    if ctx.scope == ProcessEvalScope::Run {
        Ok(())
    } else {
        Err(format!(
            "{native} cannot be used while shaping a ratchet event"
        ))
    }
}

fn process_value_is_callable(value: &EValue) -> bool {
    matches!(
        value,
        EValue::Closure(_, _)
            | EValue::Function(_)
            | EValue::NativeFunction(_)
            | EValue::HostHandle { .. }
    )
}

fn parse_process_ratchet_args(
    args: &[EValue],
) -> Result<
    (
        u32,
        crate::process::ProcessRatchetMode,
        Option<f32>,
        Option<EValue>,
    ),
    String,
> {
    if args.is_empty() {
        return Err("ratchet! requires keyword arguments".to_string());
    }
    if args.len() % 2 != 0 {
        return Err("ratchet! expects keyword/value pairs".to_string());
    }
    let mut times = None;
    let mut mode = crate::process::ProcessRatchetMode::Subdivide;
    let mut span_beats = None;
    let mut shape = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        let value = &args[idx + 1];
        match key.as_str() {
            "times" => {
                let n = process_number_arg(Some(value), "ratchet! :times")?;
                if !n.is_finite() || n < 0.0 || n.fract() != 0.0 {
                    return Err("ratchet! :times expects a non-negative integer".to_string());
                }
                if n > 1024.0 {
                    return Err("ratchet! :times must be <= 1024".to_string());
                }
                times = Some(n as u32);
            }
            "mode" => {
                let value = process_symbol_name(value)?.to_ascii_lowercase();
                mode = match value.as_str() {
                    "subdivide" => crate::process::ProcessRatchetMode::Subdivide,
                    "repeat" => crate::process::ProcessRatchetMode::Repeat,
                    _ => return Err("ratchet! :mode expects :subdivide or :repeat".to_string()),
                };
            }
            "span" => {
                let beats = process_number_arg(Some(value), "ratchet! :span")?;
                if !beats.is_finite() || beats < 0.0 {
                    return Err(
                        "ratchet! :span expects a non-negative finite beat value".to_string()
                    );
                }
                span_beats = Some(beats as f32);
            }
            "shape" => {
                if !process_value_is_callable(value) {
                    return Err("ratchet! :shape expects a callable value".to_string());
                }
                shape = Some(value.clone());
            }
            other => return Err(format!("ratchet! unknown key :{other}")),
        }
        idx += 2;
    }
    let times = times.ok_or_else(|| "ratchet! requires :times".to_string())?;
    Ok((times, mode, span_beats, shape))
}

fn process_ratchet_event_value(event: crate::process::ProcessRatchetEvent) -> EValue {
    fn number(value: impl Into<f64>) -> Rc<RefCell<EValue>> {
        Rc::new(RefCell::new(EValue::Number(value.into())))
    }

    let mut map = HashMap::new();
    map.insert(
        "offset-beats".to_string(),
        number(event.offset_beats as f64),
    );
    map.insert(
        "duration".to_string(),
        number(event.resolved.duration as f64),
    );
    map.insert(
        "velocity".to_string(),
        number(event.resolved.velocity as f64),
    );
    map.insert("speed".to_string(), number(event.resolved.speed as f64));
    map.insert("aux-a".to_string(), number(event.resolved.aux_a as f64));
    map.insert("aux-b".to_string(), number(event.resolved.aux_b as f64));
    map.insert(
        "transpose".to_string(),
        number(event.resolved.transpose as f64),
    );
    map.insert("pan".to_string(), number(event.resolved.pan as f64));
    map.insert("chop".to_string(), number(event.resolved.chop as f64));
    EValue::Map(map)
}

fn process_ratchet_event_number(
    map: &HashMap<String, Rc<RefCell<EValue>>>,
    key: &str,
) -> Result<f32, String> {
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(EValue::Number(value)) if value.is_finite() => Ok(value as f32),
        Some(other) => Err(format!(
            "ratchet event field '{key}' must be a finite number, got {}",
            eseqlisp::vm::format_lisp_value(&other)
        )),
        None => Err(format!("ratchet event missing field '{key}'")),
    }
}

fn process_ratchet_event_from_value(
    value: &EValue,
) -> Result<crate::process::ProcessRatchetEvent, String> {
    let EValue::Map(map) = value else {
        return Err("ratchet shape must return or mutate an event map".to_string());
    };
    Ok(crate::process::ProcessRatchetEvent {
        offset_beats: process_ratchet_event_number(map, "offset-beats")?,
        resolved: ResolvedStep {
            duration: process_ratchet_event_number(map, "duration")?,
            velocity: process_ratchet_event_number(map, "velocity")?,
            speed: process_ratchet_event_number(map, "speed")?,
            aux_a: process_ratchet_event_number(map, "aux-a")?,
            aux_b: process_ratchet_event_number(map, "aux-b")?,
            transpose: process_ratchet_event_number(map, "transpose")?,
            pan: process_ratchet_event_number(map, "pan")?,
            chop: process_ratchet_event_number(map, "chop")?,
        },
    })
}

fn process_ratchet_event_param_key(native: &str) -> Option<&'static str> {
    match native.trim_end_matches('!') {
        "vel" => Some("velocity"),
        "note" => Some("transpose"),
        "dur" => Some("duration"),
        "speed" => Some("speed"),
        "pan" => Some("pan"),
        "chop" => Some("chop"),
        _ => None,
    }
}

fn process_ratchet_event_read(value: Option<&EValue>, native: &str) -> Result<EValue, String> {
    let key = process_ratchet_event_param_key(native)
        .ok_or_else(|| format!("unknown ratchet event reader '{native}'"))?;
    let Some(EValue::Map(map)) = value else {
        return Err(format!("{native} expects a ratchet event map"));
    };
    Ok(EValue::Number(
        process_ratchet_event_number(map, key)? as f64
    ))
}

fn process_ratchet_event_write(args: &[EValue], native: &str) -> Result<EValue, String> {
    let key = process_ratchet_event_param_key(native)
        .ok_or_else(|| format!("unknown ratchet event writer '{native}'"))?;
    let Some(EValue::Map(map)) = args.first() else {
        return Err(format!("{native} expects an event map and number"));
    };
    let value = process_number_arg(args.get(1), native)?;
    if !value.is_finite() {
        return Err(format!("{native} expects a finite number"));
    }
    let Some(cell) = map.get(key) else {
        return Err(format!("ratchet event missing field '{key}'"));
    };
    *cell.borrow_mut() = EValue::Number(value);
    Ok(EValue::Number(value))
}

fn register_process_ratchet_event_natives(runtime: &mut Runtime) {
    for native in ["vel", "note", "dur", "speed", "pan", "chop"] {
        runtime.register_native_with_docs(
            native,
            native,
            "Read a ratchet shape event parameter.",
            move |args, _ctx| {
                if args.len() != 1 {
                    return Err(format!("{native} expects one event argument"));
                }
                process_ratchet_event_read(args.first(), native)
            },
        );
    }
    for native in ["vel!", "note!", "dur!", "speed!", "pan!", "chop!"] {
        runtime.register_native_with_docs(
            native,
            native,
            "Mutate a ratchet shape event parameter.",
            move |args, _ctx| {
                if args.len() != 2 {
                    return Err(format!("{native} expects an event and number"));
                }
                process_ratchet_event_write(&args, native)
            },
        );
    }
    runtime.register_native_with_docs(
        "nudge!",
        "(nudge! event beats)",
        "Offset a ratchet shape event by a number of beats.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("nudge! expects an event and beat offset".to_string());
            }
            let Some(EValue::Map(map)) = args.first() else {
                return Err("nudge! expects an event map and beat offset".to_string());
            };
            let amount = process_number_arg(args.get(1), "nudge!")?;
            if !amount.is_finite() {
                return Err("nudge! expects a finite beat offset".to_string());
            }
            let current = process_ratchet_event_number(map, "offset-beats")? as f64;
            let Some(cell) = map.get("offset-beats") else {
                return Err("ratchet event missing field 'offset-beats'".to_string());
            };
            let next = current + amount;
            *cell.borrow_mut() = EValue::Number(next);
            Ok(EValue::Number(next))
        },
    );
}

fn push_process_target_write(
    process_eval: &SharedProcessEvalContext,
    op: crate::process::ProcessTargetOp,
    port: Option<String>,
    value: f32,
) -> Result<(), String> {
    let mut guard = process_eval
        .lock()
        .map_err(|_| "failed to lock process eval context".to_string())?;
    let Some(ctx) = guard.as_mut() else {
        return Err("target write called outside process execution".to_string());
    };
    ensure_process_run_scope(ctx, "target write")?;
    if !ctx.conductor_play_tracks.is_empty() && ctx.step_context.is_none() {
        return Err(
            "conductors play through emit; direct target writes are not supported".to_string(),
        );
    }
    let port = match port {
        Some(port) => port,
        None => match ctx.ports.as_slice() {
            [only] => only.name.clone(),
            [] => return Err("process target write requires :target or :targets".to_string()),
            _ => {
                return Err(
                    "process has multiple target ports; target write requires an explicit port"
                        .to_string(),
                );
            }
        },
    };
    let Some(port_def) = ctx.ports.iter().find(|entry| entry.name == port).cloned() else {
        return Err(format!("unknown process target port '{port}'"));
    };
    let write = crate::process::ProcessTargetWrite {
        port: port_def.name,
        target: port_def.target,
        op,
        value,
    };
    ctx.target_writes.push(write.clone());
    ctx.commands
        .push(crate::process::ProcessRunCommand::TargetWrite(write));
    Ok(())
}

fn parse_process_def(name: &str, args: &[EValue]) -> Result<crate::process::ProcessDef, String> {
    let mut inlets = Vec::new();
    let mut outlets = Vec::new();
    let mut state = Vec::new();
    let mut every = None;
    let mut seed_policy = crate::process::ProcessSeedPolicy::default();
    let mut ports: Option<Vec<crate::process::ProcessPortDef>> = None;
    let mut doc = None;
    let mut run_value = None;
    let mut listen_value = None;
    let mut handlers: HashMap<String, EValue> = HashMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-process missing value for :{key}"));
        };
        match key.as_str() {
            "in" => inlets = parse_process_inlets(value)?,
            "out" => outlets = parse_process_outlets(value)?,
            "state" => state = parse_process_state(value)?,
            "every" => every = Some(parse_process_time_expr(value)?),
            "seed" => seed_policy = parse_process_seed_policy(value)?,
            "target" => {
                if ports.is_some() {
                    return Err("def-process cannot specify both :target and :targets".to_string());
                }
                ports = Some(vec![parse_process_default_target(value)?]);
            }
            "targets" => {
                if ports.is_some() {
                    return Err("def-process cannot specify both :target and :targets".to_string());
                }
                ports = Some(parse_process_ports(value)?);
            }
            "run" => run_value = Some(value.clone()),
            "listen" => listen_value = Some(value.clone()),
            other if other.starts_with("on-") => {
                handlers.insert(other.trim_start_matches("on-").to_string(), value.clone());
            }
            "doc" => {
                if let EValue::String(value) = value {
                    doc = Some(value.clone());
                }
            }
            "phase" | "init" => {}
            other => return Err(format!("def-process unknown key :{other}")),
        }
        idx += 1;
    }
    let state_names = state
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    let inlet_names = inlets
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    let run_source = run_value
        .as_ref()
        .map(|value| wrap_process_body_source(&state_names, &inlet_names, value))
        .transpose()?;
    let listens = listen_value
        .as_ref()
        .map(|value| parse_process_listens(value, &handlers, &state_names, &inlet_names))
        .transpose()?
        .unwrap_or_default();
    Ok(crate::process::ProcessDef {
        id: crate::process::stable_process_id(name),
        name: name.to_string(),
        source_path: None,
        doc,
        inlets,
        outlets,
        state,
        every,
        seed_policy,
        ports: ports.unwrap_or_default(),
        accumulator: None,
        run_source,
        listens,
    })
}

fn value_list(value: &EValue) -> Option<Vec<EValue>> {
    match value {
        EValue::List(items) => Some(items.iter().map(|item| item.borrow().clone()).collect()),
        _ => None,
    }
}

fn parse_process_inlets(value: &EValue) -> Result<Vec<crate::process::ProcessInletDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":in expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "inlet declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "inlet declaration missing name".to_string())?,
            )?;
            let (kind, min, max) = parse_process_inlet_kind_and_range(&items)?;
            let default = keyword_value(&items, "default").unwrap_or(EValue::Number(0.0));
            let lane = keyword_value(&items, "lane")
                .map(|value| process_truthy(&value))
                .unwrap_or(false);
            let doc = keyword_value(&items, "doc").and_then(|value| match value {
                EValue::String(value) => Some(value),
                _ => None,
            });
            Ok(crate::process::ProcessInletDef {
                name,
                kind,
                min,
                max,
                default,
                lane,
                doc,
            })
        })
        .collect()
}

fn process_truthy(value: &EValue) -> bool {
    !matches!(value, EValue::Nil | EValue::Bool(false))
}

fn parse_process_inlet_kind_and_range(
    items: &[EValue],
) -> Result<(crate::process::ProcessInletKind, Option<f32>, Option<f32>), String> {
    let Some(kind_value) = items.get(1) else {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    };
    let Ok(kind_name) = process_symbol_name(kind_value) else {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    };
    if matches!(
        kind_name.as_str(),
        "default" | "lane" | "doc" | "min" | "max"
    ) {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    }
    let kind = match kind_name.as_str() {
        "float" => crate::process::ProcessInletKind::Float,
        "int" | "integer" => crate::process::ProcessInletKind::Int,
        "gate" | "bool" | "boolean" => crate::process::ProcessInletKind::Gate,
        "track" => crate::process::ProcessInletKind::Track,
        "field" => crate::process::ProcessInletKind::Field,
        "any" => crate::process::ProcessInletKind::Any,
        other => return Err(format!("unknown process inlet kind :{other}")),
    };
    let positional_min = items.get(2).and_then(|value| match value {
        EValue::Number(value) => Some(*value as f32),
        _ => None,
    });
    let positional_max = items.get(3).and_then(|value| match value {
        EValue::Number(value) => Some(*value as f32),
        _ => None,
    });
    let min = keyword_value(items, "min")
        .and_then(|value| match value {
            EValue::Number(value) => Some(value as f32),
            _ => None,
        })
        .or(positional_min);
    let max = keyword_value(items, "max")
        .and_then(|value| match value {
            EValue::Number(value) => Some(value as f32),
            _ => None,
        })
        .or(positional_max);
    Ok((kind, min, max))
}

fn parse_process_seed_policy(value: &EValue) -> Result<crate::process::ProcessSeedPolicy, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "locked" => Ok(crate::process::ProcessSeedPolicy::Locked),
        "per-cycle" | "per_cycle" => Ok(crate::process::ProcessSeedPolicy::PerCycle),
        other => Err(format!("unknown process seed policy :{other}")),
    }
}

fn parse_process_default_target(value: &EValue) -> Result<crate::process::ProcessPortDef, String> {
    if value_list(value).is_some() {
        return Ok(crate::process::ProcessPortDef::default_with_target(
            parse_process_target_hint(value)?,
        ));
    }
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    if name == "mappable" {
        return Ok(crate::process::ProcessPortDef::default_mappable(None, None));
    }
    Err(":target expects a target hint list or :mappable".to_string())
}

fn parse_process_accumulator_target(
    value: &EValue,
) -> Result<crate::process::ProcessPortDef, String> {
    parse_process_default_target(value)
}

fn parse_process_target_kind(value: &EValue) -> Result<crate::process::ProcessTargetKind, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "step-param" | "step_param" | "step" => Ok(crate::process::ProcessTargetKind::StepParam),
        "device-param" | "device_param" | "device" => {
            Ok(crate::process::ProcessTargetKind::DeviceParam)
        }
        "instrument-param" | "instrument_param" | "instrument" => {
            Ok(crate::process::ProcessTargetKind::InstrumentParam)
        }
        "effect-param" | "effect_param" | "effect" | "fx-param" | "fx_param" => {
            Ok(crate::process::ProcessTargetKind::EffectParam)
        }
        "midi-fx-param" | "midi_fx_param" | "midi-fx" | "midi_fx" => {
            Ok(crate::process::ProcessTargetKind::MidiFxParam)
        }
        "process-inlet" | "process_inlet" | "inlet" => {
            Ok(crate::process::ProcessTargetKind::ProcessInlet)
        }
        "rack-slot-param" | "rack_slot_param" | "rack-slot" | "rack_slot" => {
            Ok(crate::process::ProcessTargetKind::RackSlotParam)
        }
        "rack-slot-instrument-param"
        | "rack_slot_instrument_param"
        | "rack-instrument-param"
        | "rack_instrument_param" => Ok(crate::process::ProcessTargetKind::RackSlotInstrumentParam),
        "rack-macro-param" | "rack_macro_param" | "rack-macro" | "rack_macro" => {
            Ok(crate::process::ProcessTargetKind::RackMacroParam)
        }
        other => Err(format!("unknown process target kind :{other}")),
    }
}

fn parse_process_port_def(items: &[EValue]) -> Result<crate::process::ProcessPortDef, String> {
    let name = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "target port declaration missing name".to_string())?,
    )?;
    if name == crate::process::DEFAULT_PROCESS_PORT {
        return Err(format!(
            "'{name}' is reserved for internal default target ports"
        ));
    }
    let tail = &items[1..];
    match tail {
        [target] if value_list(target).is_some() => Ok(
            crate::process::ProcessPortDef::with_target(name, parse_process_target_hint(target)?),
        ),
        [marker] => {
            let marker = process_symbol_name(marker)?.to_ascii_lowercase();
            match marker.as_str() {
                "mappable" => Ok(crate::process::ProcessPortDef::mappable(name, None, None)),
                "process-inlet" | "process_inlet" => {
                    Ok(crate::process::ProcessPortDef::process_inlet(name))
                }
                _ => Err(
                    "target port expects a target hint, :mappable, or :process-inlet".to_string(),
                ),
            }
        }
        [marker, value] => {
            let marker = process_symbol_name(marker)?.to_ascii_lowercase();
            if marker != "mappable" {
                return Err("target port expects :mappable before target kind or hint".to_string());
            }
            if value_list(value).is_some() {
                Ok(crate::process::ProcessPortDef::mappable(
                    name,
                    None,
                    Some(parse_process_target_hint(value)?),
                ))
            } else {
                let target_kind = parse_process_target_kind(value)?;
                if target_kind == crate::process::ProcessTargetKind::ProcessInlet {
                    return Err(
                        "process-inlet ports use (name :process-inlet) and connect!, not :mappable"
                            .to_string(),
                    );
                }
                Ok(crate::process::ProcessPortDef::mappable(
                    name,
                    Some(target_kind),
                    None,
                ))
            }
        }
        [] => Err(
            "target port declaration requires a target hint, :mappable, or :process-inlet"
                .to_string(),
        ),
        _ => Err("target port declaration has too many fields".to_string()),
    }
}

fn parse_process_ports(value: &EValue) -> Result<Vec<crate::process::ProcessPortDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":targets expects a list".to_string())?;
    let mut ports = Vec::new();
    for entry in entries {
        let items = value_list(&entry)
            .ok_or_else(|| "target port declaration must be a list".to_string())?;
        let port = parse_process_port_def(&items)?;
        if ports
            .iter()
            .any(|existing: &crate::process::ProcessPortDef| existing.name == port.name)
        {
            return Err(format!("duplicate target port '{}'", port.name));
        }
        ports.push(port);
    }
    Ok(ports)
}

fn parse_process_target_hint(value: &EValue) -> Result<crate::process::ProcessTargetHint, String> {
    let items = value_list(value).ok_or_else(|| "process target must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process target missing head".to_string())?,
    )?
    .to_ascii_lowercase();
    match head.as_str() {
        "step-param" => {
            let param = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(step-param :name) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::StepParam { param })
        }
        "param-tag" => {
            let tag = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(param-tag :tag) expects a tag".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::ParamTag { tag })
        }
        "instrument-param" => {
            let param = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(instrument-param :name) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::InstrumentParam { param })
        }
        "effect-param" => {
            let effect =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(effect-param :effect :param) expects an effect".to_string()
                })?)?;
            let param = process_symbol_name(
                items
                    .get(2)
                    .ok_or_else(|| "(effect-param :effect :param) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::EffectParam { effect, param })
        }
        "fx-param" | "midi-fx-param" | "midi-fx-target" => {
            let fx =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(midi-fx-target :fx :param) expects an fx name".to_string()
                })?)?;
            let param = process_symbol_name(
                items
                    .get(2)
                    .ok_or_else(|| "(midi-fx-target :fx :param) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::MidiFxParam { fx, param })
        }
        "rack-macro" => {
            let key =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(rack-macro :macro_1) expects a macro identifier".to_string()
                })?)?;
            let normalized = key
                .trim_start_matches(':')
                .replace('-', "_")
                .to_ascii_lowercase();
            let number = normalized
                .strip_prefix("macro_")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (1..=crate::sequencer::RACK_MACRO_COUNT).contains(value))
                .ok_or_else(|| format!("unknown rack macro :{key}"))?;
            Ok(crate::process::ProcessTargetHint::RackMacroParam {
                macro_id: (number - 1) as u8,
            })
        }
        other => Err(format!("unsupported process target {other}")),
    }
}

fn parse_process_connection_target(value: &EValue) -> Result<crate::process::ParamTarget, String> {
    let items =
        value_list(value).ok_or_else(|| "process port target must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process port target missing head".to_string())?,
    )?
    .to_ascii_lowercase();
    match head.as_str() {
        "process-inlet" => {
            if items.len() != 3 {
                return Err("(process-inlet :class :inlet) expects two arguments".to_string());
            }
            Ok(crate::process::ParamTarget::ProcessInlet {
                process: process_symbol_name(&items[1])?,
                inlet: process_symbol_name(&items[2])?,
                instance_id: None,
            })
        }
        PROCESS_INLET_INSTANCE_TARGET_TAG => {
            if items.len() != 4 {
                return Err("(inlet process :inlet) target has invalid arity".to_string());
            }
            let EValue::Number(raw_id) = items[1] else {
                return Err("(inlet process :inlet) target has invalid process id".to_string());
            };
            if !raw_id.is_finite() || raw_id < 0.0 || raw_id.fract() != 0.0 {
                return Err(
                    "(inlet process :inlet) target id must be a non-negative integer".to_string(),
                );
            }
            Ok(crate::process::ParamTarget::ProcessInlet {
                process: process_symbol_name(&items[2])?,
                inlet: process_symbol_name(&items[3])?,
                instance_id: Some(crate::process::ProcessInstanceId(raw_id as u64)),
            })
        }
        other => Err(format!(
            "connect! target must be a process inlet, got {other}"
        )),
    }
}

fn parse_process_accumulator_amount_inlet(
    value: &EValue,
) -> Result<crate::process::ProcessInletDef, String> {
    let items =
        value_list(value).ok_or_else(|| ":amount expects an inlet declaration".to_string())?;
    let name = process_symbol_name(
        items
            .first()
            .ok_or_else(|| ":amount declaration missing name".to_string())?,
    )?;
    let (kind, min, max) = parse_process_inlet_kind_and_range(&items)?;
    let default = keyword_value(&items, "default").unwrap_or(EValue::Number(0.0));
    let doc = keyword_value(&items, "doc").and_then(|value| match value {
        EValue::String(value) => Some(value),
        _ => None,
    });
    Ok(crate::process::ProcessInletDef {
        name,
        kind,
        min,
        max,
        default,
        lane: true,
        doc,
    })
}

fn parse_process_accumulator_range(value: &EValue) -> Result<(f32, f32), String> {
    let items = value_list(value).ok_or_else(|| ":range expects (lo hi)".to_string())?;
    let Some(EValue::Number(lo)) = items.first() else {
        return Err(":range expects numeric lo".to_string());
    };
    let Some(EValue::Number(hi)) = items.get(1) else {
        return Err(":range expects numeric hi".to_string());
    };
    if hi <= lo {
        return Err(":range high must be greater than low".to_string());
    }
    Ok((*lo as f32, *hi as f32))
}

fn parse_process_accumulator_mode(
    value: &EValue,
) -> Result<crate::process::ProcessAccumulatorMode, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "wrap" => Ok(crate::process::ProcessAccumulatorMode::Wrap),
        "clip" => Ok(crate::process::ProcessAccumulatorMode::Clip),
        "bounce" => Ok(crate::process::ProcessAccumulatorMode::Bounce),
        other => Err(format!("unknown def-accumulator mode :{other}")),
    }
}

fn parse_process_outlets(value: &EValue) -> Result<Vec<crate::process::ProcessOutletDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":out expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "outlet declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "outlet declaration missing name".to_string())?,
            )?;
            Ok(crate::process::ProcessOutletDef { name })
        })
        .collect()
}

fn parse_process_state(value: &EValue) -> Result<Vec<crate::process::ProcessStateDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":state expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "state declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "state declaration missing name".to_string())?,
            )?;
            let initial = items.get(1).cloned().unwrap_or(EValue::Number(0.0));
            Ok(crate::process::ProcessStateDef { name, initial })
        })
        .collect()
}

fn parse_process_listens(
    value: &EValue,
    handlers: &HashMap<String, EValue>,
    state_names: &[String],
    inlet_names: &[String],
) -> Result<Vec<crate::process::ProcessListenDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":listen expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "listen declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "listen declaration missing name".to_string())?,
            )?;
            let source_value = items
                .get(1)
                .ok_or_else(|| "listen declaration missing event source".to_string())?;
            let source = parse_process_event_source(source_value)?;
            let handler = handlers
                .get(&name)
                .ok_or_else(|| format!("listen :{name} missing :on-{name} handler"))?;
            Ok(crate::process::ProcessListenDef {
                name,
                source,
                handler_source: wrap_process_handler_source(state_names, inlet_names, handler)?,
            })
        })
        .collect()
}

fn parse_process_event_source(
    value: &EValue,
) -> Result<crate::process::ProcessEventSource, String> {
    if matches!(
        value,
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_)
    ) {
        return Ok(crate::process::ProcessEventSource::Channel(
            process_symbol_name(value)?,
        ));
    }
    let items = value_list(value).ok_or_else(|| "event source must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "event source missing head".to_string())?,
    )?;
    match head.as_str() {
        "track-fires" => {
            let Some(EValue::Number(track)) = items.get(1) else {
                return Err("track-fires expects a track number".to_string());
            };
            Ok(crate::process::ProcessEventSource::TrackFires(
                *track as usize,
            ))
        }
        "seq-fires" => {
            let name = items
                .get(1)
                .map(process_symbol_name)
                .transpose()?
                .unwrap_or_default();
            Ok(crate::process::ProcessEventSource::SeqFires(name))
        }
        "chan" | "channel" => {
            let name = items
                .get(1)
                .map(process_symbol_name)
                .transpose()?
                .unwrap_or_default();
            Ok(crate::process::ProcessEventSource::Channel(name))
        }
        other => Err(format!("unknown process event source {other}")),
    }
}

fn keyword_value(items: &[EValue], key: &str) -> Option<EValue> {
    let mut idx = 0;
    while idx + 1 < items.len() {
        if process_symbol_name(&items[idx])
            .ok()
            .is_some_and(|name| name.eq_ignore_ascii_case(key))
        {
            return Some(items[idx + 1].clone());
        }
        idx += 1;
    }
    None
}

fn parse_process_time_expr(value: &EValue) -> Result<crate::process::ProcessTimeExpr, String> {
    match value {
        EValue::Number(beats) => Ok(crate::process::ProcessTimeExpr::Beats(*beats)),
        EValue::Keyword(_) | EValue::Symbol(_) | EValue::String(_) => {
            let tb = parse_timebase_arg(std::slice::from_ref(value), 0)?;
            Ok(crate::process::ProcessTimeExpr::Beats(tb.step_beats(
                crate::generator::GENERATOR_RESOLUTION_REF_STEPS,
            )))
        }
        EValue::List(_) => {
            let items =
                value_list(value).ok_or_else(|| "time expression must be a list".to_string())?;
            let head = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "time expression missing head".to_string())?,
            )?;
            match head.as_str() {
                "beats" => {
                    let Some(EValue::Number(beats)) = items.get(1) else {
                        return Err("(beats n) expects a number".to_string());
                    };
                    Ok(crate::process::ProcessTimeExpr::Beats(*beats))
                }
                "bars" => {
                    let Some(EValue::Number(bars)) = items.get(1) else {
                        return Err("(bars n) expects a number".to_string());
                    };
                    Ok(crate::process::ProcessTimeExpr::Beats(*bars * 4.0))
                }
                "in" => {
                    let inlet = process_symbol_name(
                        items
                            .get(1)
                            .ok_or_else(|| "(in :name) expects an inlet".to_string())?,
                    )?;
                    Ok(crate::process::ProcessTimeExpr::Inlet(inlet))
                }
                other => Err(format!("unsupported process time expression {other}")),
            }
        }
        _ => Err("unsupported process time expression".to_string()),
    }
}

fn wrap_process_source_with_bindings(
    state_names: &[String],
    inlet_names: &[String],
    extra_bindings: Vec<(String, String)>,
    body_source: String,
) -> Result<String, String> {
    let mut seen = BTreeSet::new();
    let mut params = Vec::new();
    let mut args = Vec::new();
    for name in state_names {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name.clone());
        args.push(format!("(__process-state-get :{name})"));
    }
    for name in inlet_names {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name.clone());
        args.push(format!("(in :{name})"));
    }
    for (name, expr) in extra_bindings {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name);
        args.push(expr);
    }
    if params.is_empty() {
        return Ok(body_source);
    }
    let stores = state_names
        .iter()
        .map(|name| format!("(__process-state-set! :{name} {name})"))
        .collect::<Vec<_>>();
    let body_with_stores = if stores.is_empty() {
        body_source
    } else {
        format!("(do {body_source} {})", stores.join(" "))
    };
    Ok(format!(
        "((lambda ({}) {}) {})",
        params.join(" "),
        body_with_stores,
        args.join(" ")
    ))
}

fn wrap_process_body_source(
    state_names: &[String],
    inlet_names: &[String],
    body: &EValue,
) -> Result<String, String> {
    wrap_process_source_with_bindings(
        state_names,
        inlet_names,
        Vec::new(),
        eseqlisp::vm::format_lisp_source(body),
    )
}

fn wrap_process_handler_source(
    state_names: &[String],
    inlet_names: &[String],
    handler: &EValue,
) -> Result<String, String> {
    let items =
        value_list(handler).ok_or_else(|| "process event handler expects a lambda".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process event handler lambda is empty".to_string())?,
    )?;
    if head != "lambda" {
        return Err("process event handler expects a lambda".to_string());
    }
    let args = value_list(
        items
            .get(1)
            .ok_or_else(|| "process event handler lambda missing args".to_string())?,
    )
    .ok_or_else(|| "process event handler lambda args must be a list".to_string())?;
    if args.len() != 1 {
        return Err("process event handler lambda expects exactly one argument".to_string());
    }
    let event_arg = process_symbol_name(&args[0])?;
    let body = items
        .get(2..)
        .ok_or_else(|| "process event handler lambda missing body".to_string())?;
    let body_source = process_body_source(body)?;
    wrap_process_source_with_bindings(
        state_names,
        inlet_names,
        vec![(event_arg, "(__process-event-value)".to_string())],
        body_source,
    )
}

fn process_body_source(body: &[EValue]) -> Result<String, String> {
    match body {
        [] => Err("process body cannot be empty".to_string()),
        [single] => Ok(eseqlisp::vm::format_lisp_source(single)),
        forms => Ok(format!(
            "(do {})",
            forms
                .iter()
                .map(eseqlisp::vm::format_lisp_source)
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

fn construct_process_instance(
    process_authoring: &SharedProcessAuthoring,
    class_name: &str,
    args: Vec<EValue>,
    anonymous: bool,
    running: bool,
    every: Option<crate::process::ProcessTimeExpr>,
    run_source: Option<String>,
    one_shot: bool,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let constructor_args = parse_process_constructor_args(process_authoring, &args)?;
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == class_name)
        .ok_or_else(|| format!("unknown process class '{class_name}'"))?;
    validate_process_port_bindings(def, &constructor_args.bindings)?;
    let handle_id = crate::process::AuthoredHandleId(registry.next_id());
    registry.upsert_instance(crate::process::AuthoredProcessInstance {
        handle_id,
        name: None,
        class_name: class_name.to_string(),
        inlets: constructor_args.inlets,
        bindings: constructor_args.bindings,
        running,
        anonymous,
        one_shot,
        every,
        run_source,
    });
    Ok(process_instance_handle(
        Arc::clone(process_authoring),
        handle_id,
        process_chain_state,
        publish,
    ))
}

fn construct_anonymous_listener_process(
    process_authoring: &SharedProcessAuthoring,
    kind: &str,
    source: crate::process::ProcessEventSource,
    handler: &EValue,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let handler_source = wrap_process_handler_source(&[], &[], handler)?;
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let handle_id = crate::process::AuthoredHandleId(registry.next_id());
    let class_name = format!("__anonymous_{kind}_{}", handle_id.0);
    registry.upsert_def(crate::process::ProcessDef {
        id: crate::process::stable_process_id(&class_name),
        name: class_name.clone(),
        source_path: None,
        doc: None,
        inlets: Vec::new(),
        outlets: Vec::new(),
        state: Vec::new(),
        every: None,
        seed_policy: crate::process::ProcessSeedPolicy::default(),
        ports: Vec::new(),
        accumulator: None,
        run_source: None,
        listens: vec![crate::process::ProcessListenDef {
            name: "event".to_string(),
            source,
            handler_source,
        }],
    });
    registry.upsert_instance(crate::process::AuthoredProcessInstance {
        handle_id,
        name: None,
        class_name,
        inlets: HashMap::new(),
        bindings: BTreeMap::new(),
        running: true,
        anonymous: true,
        one_shot: false,
        every: None,
        run_source: None,
    });
    drop(registry);
    Ok(process_instance_handle(
        Arc::clone(process_authoring),
        handle_id,
        None,
        publish,
    ))
}

struct ProcessConstructorArgs {
    inlets: HashMap<String, crate::process::ProcessInletValue>,
    bindings: BTreeMap<String, Option<crate::process::ParamTarget>>,
}

fn parse_process_constructor_args(
    process_authoring: &SharedProcessAuthoring,
    args: &[EValue],
) -> Result<ProcessConstructorArgs, String> {
    if args.len() % 2 != 0 {
        return Err("process constructor expects keyword/value inlet pairs".to_string());
    }
    let mut inlets = HashMap::new();
    let mut bindings = BTreeMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?;
        if key.eq_ignore_ascii_case("connect") {
            bindings.extend(parse_process_constructor_connections(&args[idx + 1])?);
        } else if key.eq_ignore_ascii_case("map") {
            return Err(
                "process constructor :map was replaced by :connect for process-inlet connections"
                    .to_string(),
            );
        } else {
            let value = parse_inlet_value(process_authoring, &args[idx + 1])?;
            inlets.insert(key, value);
        }
        idx += 2;
    }
    Ok(ProcessConstructorArgs { inlets, bindings })
}

fn parse_process_constructor_connections(
    value: &EValue,
) -> Result<BTreeMap<String, Option<crate::process::ParamTarget>>, String> {
    let entries = value_list(value)
        .ok_or_else(|| "process constructor :connect expects a list".to_string())?;
    let mut bindings = BTreeMap::new();
    for entry in entries {
        let items = value_list(&entry)
            .ok_or_else(|| "process constructor :connect entries must be lists".to_string())?;
        if items.len() != 2 {
            return Err("process constructor :connect entries expect (port target)".to_string());
        }
        let port = process_symbol_name(&items[0])?;
        let target = parse_process_connection_target(&items[1])?;
        bindings.insert(port, Some(target));
    }
    Ok(bindings)
}

fn parse_inlet_value(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessInletValue, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "process-outlet" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let outlet = registry
                .outlet_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown process outlet handle".to_string())?;
            Ok(crate::process::ProcessInletValue::Outlet(outlet))
        }
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessInletValue::Channel(name))
        }
        _ => Ok(crate::process::ProcessInletValue::Literal(value.clone())),
    }
}

fn process_instance_handle(
    process_authoring: SharedProcessAuthoring,
    handle_id: crate::process::AuthoredHandleId,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> EValue {
    EValue::HostHandle {
        kind: "process".to_string(),
        id: handle_id.0,
        callable: Rc::new(move |args, _vm| {
            let read_only_inline_poll = matches!(
                args.as_slice(),
                [EValue::Keyword(command) | EValue::Symbol(command), _]
                    if command.trim_start_matches(':') == "__inline-read"
            );
            let publish_after_call = args.len() == 2 && !read_only_inline_poll;
            match process_handle_call(
                &process_authoring,
                process_chain_state.as_ref(),
                handle_id,
                args,
            ) {
                Ok(value) => {
                    if publish_after_call {
                        publish_process_authoring(&process_authoring, &publish);
                    }
                    value
                }
                Err(error) => {
                    eprintln!("[process] handle error: {error}");
                    EValue::Bool(false)
                }
            }
        }),
    }
}

fn process_channel_handle(
    process_authoring: SharedProcessAuthoring,
    handle_id: crate::process::AuthoredHandleId,
) -> EValue {
    EValue::HostHandle {
        kind: "channel".to_string(),
        id: handle_id.0,
        callable: Rc::new(move |_args, _vm| EValue::Bool(true)),
    }
}

fn process_outlet_handle(
    process_authoring: SharedProcessAuthoring,
    outlet: crate::process::ProcessOutletRef,
) -> Result<EValue, String> {
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let handle_id = registry.next_id();
    registry.outlet_handles.insert(handle_id, outlet);
    Ok(EValue::HostHandle {
        kind: "process-outlet".to_string(),
        id: handle_id,
        callable: Rc::new(move |_args, _vm| EValue::Bool(true)),
    })
}

enum DurableProcessHandleUpdate {
    Scalar(crate::process::ProcessLiteral),
    Lane(Vec<f32>),
    None,
}

fn process_instance_lane_backed_inlet(
    registry: &ProcessAuthoringRegistry,
    handle_id: crate::process::AuthoredHandleId,
    inlet: &str,
) -> Result<bool, String> {
    let instance = registry
        .instances
        .iter()
        .find(|entry| entry.handle_id == handle_id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == instance.class_name)
        .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
    Ok(def
        .inlets
        .iter()
        .any(|entry| entry.name == inlet && entry.lane))
}

fn process_handle_call(
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<&Arc<crate::sequencer::SequencerState>>,
    handle_id: crate::process::AuthoredHandleId,
    args: Vec<EValue>,
) -> Result<EValue, String> {
    if let [EValue::Keyword(command), key] = args.as_slice() {
        if command != "__inline-read" {
            // Fall through to the public process-handle call forms below.
        } else {
            let inlet = process_symbol_name(key)?;
            if let Some(value) = process_chain_state.and_then(|state| {
                state.process_inlet_value(crate::process::ProcessInstanceId(handle_id.0), &inlet)
            }) {
                return Ok(value.to_value());
            }
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            return Ok(registry
                .instances
                .iter()
                .find(|instance| instance.handle_id == handle_id)
                .and_then(|instance| instance.inlets.get(&inlet))
                .and_then(|value| match value {
                    crate::process::ProcessInletValue::Literal(value) => Some(value.clone()),
                    _ => None,
                })
                .unwrap_or(EValue::Nil));
        }
    }
    match args.as_slice() {
        [key] => {
            let outlet = process_symbol_name(key)?;
            process_outlet_handle(
                Arc::clone(process_authoring),
                crate::process::ProcessOutletRef {
                    process_handle_id: handle_id,
                    outlet,
                },
            )
        }
        [key, value] => {
            let inlet = process_symbol_name(key)?;
            let value = parse_inlet_value(process_authoring, value)?;
            let attachment_count = process_chain_state
                .map(|state| {
                    state.process_instance_attachment_count(crate::process::ProcessInstanceId(
                        handle_id.0,
                    ))
                })
                .unwrap_or(0);
            let durable_update = match &value {
                crate::process::ProcessInletValue::Literal(literal) => {
                    if let Some(values) = process_lane_values(literal) {
                        DurableProcessHandleUpdate::Lane(values?)
                    } else {
                        DurableProcessHandleUpdate::Scalar(
                            crate::process::ProcessLiteral::from_value(literal)?,
                        )
                    }
                }
                _ if attachment_count > 0 => {
                    return Err(
                        "attached process chain inlets must be literals; use process graphs outside `processes` for outlet/channel wiring"
                            .to_string(),
                    );
                }
                _ => DurableProcessHandleUpdate::None,
            };
            let mut registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            if matches!(durable_update, DurableProcessHandleUpdate::Lane(_))
                && !process_instance_lane_backed_inlet(&registry, handle_id, &inlet)?
            {
                return Err(format!("inlet '{inlet}' is not lane-backed (:lane true)"));
            }
            let instance = registry
                .instances
                .iter_mut()
                .find(|entry| entry.handle_id == handle_id)
                .ok_or_else(|| "unknown process handle".to_string())?;
            instance.inlets.insert(inlet.clone(), value);
            drop(registry);
            if let Some(state) = process_chain_state {
                match durable_update {
                    DurableProcessHandleUpdate::Scalar(literal) => {
                        state.set_process_inlet_value(
                            crate::process::ProcessInstanceId(handle_id.0),
                            &inlet,
                            literal,
                        );
                    }
                    DurableProcessHandleUpdate::Lane(values) => {
                        state.set_process_lane_values(
                            crate::process::ProcessInstanceId(handle_id.0),
                            &inlet,
                            values,
                        );
                    }
                    DurableProcessHandleUpdate::None => {}
                }
            }
            Ok(EValue::Bool(true))
        }
        _ => Err("process handle expects :outlet or :inlet value".to_string()),
    }
}

fn set_process_running(
    process_authoring: &SharedProcessAuthoring,
    value: Option<&EValue>,
    running: bool,
) -> Result<(), String> {
    let Some(EValue::HostHandle { kind, id, .. }) = value else {
        return Err("start/stop expects a process handle".to_string());
    };
    if kind != "process" {
        return Err("start/stop expects a process handle".to_string());
    }
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let instance = registry
        .instances
        .iter_mut()
        .find(|entry| entry.handle_id.0 == *id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    instance.running = running;
    Ok(())
}

fn parse_channel_name(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<String, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())
        }
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_) => process_symbol_name(value),
        _ => Err("expected channel handle or name".to_string()),
    }
}

fn parse_process_source_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessSourceRef, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "process-outlet" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let outlet = registry
                .outlet_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown process outlet handle".to_string())?;
            Ok(crate::process::ProcessSourceRef::Outlet(outlet))
        }
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessSourceRef::Channel(name))
        }
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_) => Ok(
            crate::process::ProcessSourceRef::Channel(process_symbol_name(value)?),
        ),
        EValue::List(items) => {
            let items = items
                .iter()
                .map(|item| item.borrow().clone())
                .collect::<Vec<_>>();
            let head = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "source expression missing head".to_string())?,
            )?;
            match head.as_str() {
                "chan" | "channel" => {
                    let name = process_symbol_name(
                        items
                            .get(1)
                            .ok_or_else(|| "channel source expects a name".to_string())?,
                    )?;
                    Ok(crate::process::ProcessSourceRef::Channel(name))
                }
                process_name => {
                    if items.len() != 2 {
                        return Err(
                            "process outlet source expects (process-name :outlet)".to_string()
                        );
                    }
                    let outlet = process_symbol_name(&items[1])?;
                    let registry = process_authoring
                        .lock()
                        .map_err(|_| "failed to lock process registry".to_string())?;
                    let instance = registry
                        .instances
                        .iter()
                        .rev()
                        .find(|entry| entry.name.as_deref() == Some(process_name))
                        .ok_or_else(|| format!("unknown process instance {process_name}"))?;
                    Ok(crate::process::ProcessSourceRef::Outlet(
                        crate::process::ProcessOutletRef {
                            process_handle_id: instance.handle_id,
                            outlet,
                        },
                    ))
                }
            }
        }
        _ => Err("source must be a process outlet or channel".to_string()),
    }
}

fn parse_process_event_source_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessEventSource, String> {
    if let Ok(source) = parse_process_source_ref(process_authoring, value) {
        return Ok(match source {
            crate::process::ProcessSourceRef::Channel(name) => {
                crate::process::ProcessEventSource::Channel(name)
            }
            crate::process::ProcessSourceRef::Outlet(outlet) => {
                crate::process::ProcessEventSource::Outlet(outlet)
            }
            crate::process::ProcessSourceRef::TrackFires(track) => {
                crate::process::ProcessEventSource::TrackFires(track)
            }
            crate::process::ProcessSourceRef::SeqFires(name) => {
                crate::process::ProcessEventSource::SeqFires(name)
            }
        });
    }
    parse_process_event_source(value)
}

fn parse_process_target_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessTargetRef, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessTargetRef::Channel(name))
        }
        EValue::List(_) => {
            Err("explicit inlet target handles are not represented as lists".to_string())
        }
        _ => Err(
            "target must be a channel in v1 or an inlet patch through constructor nesting"
                .to_string(),
        ),
    }
}

fn build_process_emit_event(args: &[EValue]) -> Result<EmittedAccumulatorEvent, String> {
    let mut idx = 0;
    if args
        .first()
        .is_some_and(|value| !matches!(value, EValue::Keyword(_)))
    {
        idx = 1;
    }
    let mut resolved = crate::generator::default_resolved();
    let mut track = None;
    let mut offset_beats = 0.0_f32;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("emit missing value for :{key}"));
        };
        match (key.as_str(), value) {
            ("track", EValue::Number(value)) => track = Some((*value).max(0.0) as usize),
            ("after", value) => {
                offset_beats = parse_process_time_expr(value)?.beats(&HashMap::new()) as f32
            }
            ("note" | "transpose", EValue::Number(value)) => resolved.transpose = *value as f32,
            ("vel" | "velocity", EValue::Number(value)) => resolved.velocity = *value as f32,
            ("duration" | "dur", EValue::Number(value)) => resolved.duration = *value as f32,
            ("speed", EValue::Number(value)) => resolved.speed = *value as f32,
            ("pan", EValue::Number(value)) => resolved.pan = *value as f32,
            ("chop", EValue::Number(value)) => resolved.chop = *value as f32,
            _ => {}
        }
        idx += 1;
    }
    Ok(EmittedAccumulatorEvent {
        offset_beats,
        track,
        resolved,
        chord: Vec::new(),
        chord_durations: Vec::new(),
        chord_step_transpose: 0.0,
        effect_params: Vec::new(),
        instrument_params: Vec::new(),
    })
}

fn process_status_value(registry: &ProcessAuthoringRegistry) -> EValue {
    process_map([
        (
            "defs",
            process_list(registry.defs.iter().map(|def| {
                process_map([
                    ("id", EValue::Number(def.id as f64)),
                    ("name", EValue::String(def.name.clone())),
                    (
                        "inlets",
                        process_string_list(def.inlets.iter().map(|inlet| &inlet.name)),
                    ),
                    (
                        "outlets",
                        process_string_list(def.outlets.iter().map(|outlet| &outlet.name)),
                    ),
                    (
                        "state",
                        process_string_list(def.state.iter().map(|cell| &cell.name)),
                    ),
                    ("run-source", EValue::Bool(def.run_source.is_some())),
                ])
            })),
        ),
        (
            "instances",
            process_list(registry.instances.iter().map(|instance| {
                process_map([
                    ("id", EValue::Number(instance.handle_id.0 as f64)),
                    (
                        "name",
                        instance
                            .name
                            .as_ref()
                            .map(|name| EValue::String(name.clone()))
                            .unwrap_or(EValue::Nil),
                    ),
                    ("class", EValue::String(instance.class_name.clone())),
                    ("running", EValue::Bool(instance.running)),
                    ("anonymous", EValue::Bool(instance.anonymous)),
                    ("one-shot", EValue::Bool(instance.one_shot)),
                    (
                        "inlets",
                        process_list(instance.inlets.iter().map(|(name, value)| {
                            process_map([
                                ("name", EValue::String(name.clone())),
                                ("value", process_inlet_status_value(value)),
                            ])
                        })),
                    ),
                ])
            })),
        ),
        (
            "channels",
            process_list(registry.channels.iter().map(|channel| {
                process_map([
                    ("id", EValue::Number(channel.handle_id.0 as f64)),
                    (
                        "name",
                        channel
                            .name
                            .as_ref()
                            .map(|name| EValue::String(name.clone()))
                            .unwrap_or(EValue::Nil),
                    ),
                    ("message-only", EValue::Bool(channel.message_only)),
                    ("initial", channel.initial.clone().unwrap_or(EValue::Nil)),
                ])
            })),
        ),
        ("patches", EValue::Number(registry.patches.len() as f64)),
        (
            "listeners",
            EValue::Number(
                registry
                    .defs
                    .iter()
                    .map(|def| def.listens.len())
                    .sum::<usize>() as f64,
            ),
        ),
    ])
}

fn process_inlet_status_value(value: &crate::process::ProcessInletValue) -> EValue {
    match value {
        crate::process::ProcessInletValue::Literal(value) => process_map([
            ("kind", EValue::Keyword("literal".to_string())),
            ("value", value.clone()),
        ]),
        crate::process::ProcessInletValue::Channel(name) => process_map([
            ("kind", EValue::Keyword("channel".to_string())),
            ("name", EValue::String(name.clone())),
        ]),
        crate::process::ProcessInletValue::Outlet(outlet) => process_map([
            ("kind", EValue::Keyword("outlet".to_string())),
            (
                "process-id",
                EValue::Number(outlet.process_handle_id.0 as f64),
            ),
            ("outlet", EValue::String(outlet.outlet.clone())),
        ]),
    }
}

fn process_string_list<'a>(items: impl IntoIterator<Item = &'a String>) -> EValue {
    process_list(items.into_iter().map(|item| EValue::String(item.clone())))
}

fn process_list(items: impl IntoIterator<Item = EValue>) -> EValue {
    EValue::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

fn process_map(items: impl IntoIterator<Item = (&'static str, EValue)>) -> EValue {
    EValue::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
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
            let pattern_idx = state_for_neural_select.current_scene_index();
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
            let pattern_idx = state_for_neural_selected_predicate.current_scene_index();
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
            let pattern_idx = state_for_neural_delete.current_scene_index();
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
        "(beats :8) | (beats 0.25)",
        "Beats in one step of a timebase, or a numeric beat duration.",
        move |args, _ctx| {
            if let Some(EValue::Number(n)) = args.first() {
                return Ok(EValue::Number(n.max(0.0)));
            }
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
        "(midi-fx-param \"name\" :default value :min value :max value :role symbol :enum \"a\" \"b\" ...)",
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
                        tensor_params: Vec::new(),
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

    let fx_eval_for_velocity = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-velocity",
        "(fx-velocity)",
        "Return the resolved velocity for the current MIDI FX event.",
        move |_args, _ctx| eval_midi_fx_velocity(&fx_eval_for_velocity),
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

fn eval_midi_fx_velocity(
    accumulator_eval: &SharedAccumulatorEvalContext,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    Ok(EValue::Number(eval.resolved.velocity as f64))
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

pub fn published_sequencer_from_def_args(args: &[EValue]) -> Result<PublishedSequencer, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    if graph_mode_present(args) {
        let manifest = parse_graph_manifest(args)?;
        return Ok(PublishedSequencer {
            id: manifest.id,
            name,
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });
    }

    let mut resolution: u8 = Timebase::Sixteenth as u8;
    let mut tick_source: Option<String> = None;
    let mut idx = 1;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("def-sequencer expects keyword/value pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-sequencer missing value for :{key}"));
        };
        match key.as_str() {
            "resolution" | "res" => resolution = sequencer_resolution_index(value),
            "tick" => tick_source = Some(sequencer_tick_source(value)),
            "init" => { /* reserved for future one-time init */ }
            _ => return Err(format!("def-sequencer unknown key :{key}")),
        }
        idx += 1;
    }
    let Some(tick_source) = tick_source else {
        return Err("def-sequencer requires :tick".to_string());
    };
    Ok(PublishedSequencer {
        id: stable_sequencer_id(&name),
        name,
        resolution,
        tick_source,
        graph: None,
    })
}

fn gen_splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
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

fn midi_fx_known_param_attr_name(value: &EValue) -> Option<String> {
    let name = midi_fx_attr_name(value)?;
    matches!(
        name.as_str(),
        "default" | "min" | "max" | "unit" | "role" | "tags" | "enum"
    )
    .then_some(name)
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
    let mut role = None;
    let mut tags = Vec::new();
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
            "role" => {
                role = match args.get(idx) {
                    Some(EValue::String(value))
                    | Some(EValue::Keyword(value))
                    | Some(EValue::Symbol(value)) => Some(value.clone()),
                    _ => return Err("midi-fx-param :role expects string/symbol".to_string()),
                };
                idx += 1;
            }
            "tags" => {
                while idx < args.len() && midi_fx_known_param_attr_name(&args[idx]).is_none() {
                    match &args[idx] {
                        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                            tags.push(value.clone())
                        }
                        _ => return Err("midi-fx-param :tags values must be strings".to_string()),
                    }
                    idx += 1;
                }
            }
            "enum" => {
                let mut enum_labels = Vec::new();
                while idx < args.len() && midi_fx_known_param_attr_name(&args[idx]).is_none() {
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
        ui_metadata: crate::effects::ParamUiMetadata::with_tags(None, None, role, tags),
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
    let current_pattern = state.current_scene_index();
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
    let current_pattern = state.current_scene_index();
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
    let current_pattern = state.current_scene_index();
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
    let current_pattern = state.current_scene_index();
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
                            HostCommand::AuthoringTransactionBegin { .. }
                            | HostCommand::AuthoringTransactionEnd { .. } => {}
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
        process_authoring,
        process_eval,
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
                    process_authoring,
                    process_eval,
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
    scratch_runtime_with_fallbacks_inner(state, track, cursor_step, true)
}

pub fn scheduler_scratch_runtime_with_fallbacks(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
) -> ScratchControlRuntime {
    scratch_runtime_with_fallbacks_inner(state, track, cursor_step, false)
}

fn scratch_runtime_with_fallbacks_inner(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
    write_process_chain_state: bool,
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
    let mut runtime = ScratchControlRuntime::new_with_process_chain_writes(
        state,
        effect_descriptors,
        instrument_descriptors,
        track,
        cursor_step,
        write_process_chain_state,
    );
    runtime.set_theme_sync_enabled(false);
    runtime
}

fn midi_fx_library_root_candidates() -> Vec<PathBuf> {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("midi-fx");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = [
        cwd.join("midi-fx"),
        cwd.join("crates").join("sequencer").join("midi-fx"),
        manifest_root,
    ];
    let mut unique = Vec::new();
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        let canonical = candidate.canonicalize().unwrap_or(candidate);
        if !unique.iter().any(|existing| existing == &canonical) {
            unique.push(canonical);
        }
    }
    unique
}

fn midi_fx_name_components(name: &str) -> Option<Vec<&str>> {
    let trimmed = name.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut components = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(value) => components.push(value.to_str()?),
            _ => return None,
        }
    }
    if components.is_empty() {
        None
    } else {
        Some(components)
    }
}

fn midi_fx_source_path(name: &str) -> Option<PathBuf> {
    let components = midi_fx_name_components(name)?;
    for root in midi_fx_library_root_candidates() {
        let mut folder = root.clone();
        for component in &components {
            folder.push(component);
        }
        let folder_dsp = folder.join("dsp.lisp");
        if folder_dsp.exists() {
            return Some(folder_dsp);
        }

        let mut file = root;
        for component in &components[..components.len().saturating_sub(1)] {
            file.push(component);
        }
        file.push(format!("{}.lisp", components[components.len() - 1]));
        if file.exists() {
            return Some(file);
        }
    }
    None
}

fn load_midi_fx_source(name: &str) -> io::Result<String> {
    let path = midi_fx_source_path(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "MIDI FX source not found"))?;
    std::fs::read_to_string(path)
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

    let mut sources = Vec::new();
    for root in midi_fx_library_root_candidates() {
        collect(&root, &root, &mut sources);
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
        .into_iter()
        .map(|(name, src)| format!("; midi-fx/{name}/dsp.lisp\n{src}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn load_process_library_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("processes")
        .join("builtin.lisp");
    match std::fs::read_to_string(&path) {
        Ok(source) if source.trim().is_empty() => String::new(),
        Ok(_) => {
            // Evaluate through `load` so def-process can retain the VM's real
            // source module for slot-to-source navigation. In packaged builds
            // where the source tree is unavailable, retain the embedded
            // definitions as a functional fallback.
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            format!("(load {:?})", path.to_string_lossy())
        }
        Err(_) => {
            let source = include_str!("../processes/builtin.lisp");
            if source.trim().is_empty() {
                String::new()
            } else {
                format!("; embedded processes/builtin.lisp\n{source}\n")
            }
        }
    }
}

fn load_midi_fx_descriptors_from_source(source: String) -> Vec<EffectDescriptor> {
    if source.trim().is_empty() {
        return Vec::new();
    }

    let cache = MIDI_FX_DESCRIPTOR_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().expect("midi fx descriptor cache poisoned");
        if let Some(cached) = guard.get(&source) {
            return cached.clone();
        }
    }

    if let Ok(descriptors) = parse_midi_fx_descriptors_from_source(&source) {
        if !descriptors.is_empty() {
            cache
                .lock()
                .expect("midi fx descriptor cache poisoned")
                .insert(source, descriptors.clone());
            return descriptors;
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
    cache
        .lock()
        .expect("midi fx descriptor cache poisoned")
        .insert(source, descriptors.clone());
    descriptors
}

fn parse_midi_fx_descriptors_from_source(source: &str) -> Result<Vec<EffectDescriptor>, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize MIDI FX source: {error:?}"))?;
    let expressions = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse MIDI FX source: {error:?}"))?;
    let mut pending_params = Vec::new();
    let mut descriptors: Vec<EffectDescriptor> = Vec::new();

    for expression in expressions {
        let Expression::List(items) = expression else {
            continue;
        };
        let Some(Expression::Symbol(operator)) = items.first() else {
            continue;
        };
        match operator.as_str() {
            "midi-fx-param" => {
                let Some(name) = items.get(1).and_then(midi_fx_metadata_name) else {
                    return Err("midi-fx-param expects a name".to_string());
                };
                let args = items[2..]
                    .iter()
                    .map(midi_fx_metadata_value)
                    .collect::<Result<Vec<_>, _>>()?;
                pending_params.push(parse_midi_fx_param_descriptor(&name, &args)?);
            }
            "def-midi-fx" => {
                let Some(name) = items.get(1).and_then(midi_fx_metadata_name) else {
                    return Err("def-midi-fx expects a name".to_string());
                };
                let mut params = std::mem::take(&mut pending_params);
                ensure_enabled_param(&mut params);
                for (idx, param) in params.iter_mut().enumerate() {
                    param.node_param_idx = idx as u32;
                }
                let mut descriptor = EffectDescriptor::empty_custom_slot();
                descriptor.name = name.clone();
                descriptor.params = params;
                if let Some(existing) = descriptors.iter_mut().find(|desc| desc.name == name) {
                    *existing = descriptor;
                } else {
                    descriptors.push(descriptor);
                }
            }
            _ => {}
        }
    }

    Ok(descriptors)
}

fn midi_fx_metadata_name(expression: &Expression) -> Option<String> {
    match expression {
        Expression::String(name) | Expression::Symbol(name) | Expression::Keyword(name) => {
            Some(name.trim_start_matches('@').to_string())
        }
        _ => None,
    }
}

fn midi_fx_metadata_value(expression: &Expression) -> Result<EValue, String> {
    match expression {
        Expression::String(value) => Ok(EValue::String(value.clone())),
        Expression::Symbol(value) => Ok(EValue::Symbol(value.clone())),
        Expression::Keyword(value) => Ok(EValue::Keyword(value.clone())),
        Expression::Number(value) => Ok(EValue::Number(*value)),
        _ => Err("MIDI FX metadata supports only literal parameter attributes".to_string()),
    }
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

pub fn process_library_source_with_user_source(user_source: &str) -> String {
    let library = load_process_library_source();
    if library.trim().is_empty() {
        user_source.to_string()
    } else if user_source.trim().is_empty() {
        library
    } else {
        format!("{library}\n; *scratch*\n{user_source}")
    }
}

pub fn load_midi_fx_descriptors() -> Vec<EffectDescriptor> {
    load_midi_fx_descriptors_from_source(load_midi_fx_library_source())
}

pub fn load_midi_fx_descriptor(name: &str) -> Option<EffectDescriptor> {
    if let Ok(source) = load_midi_fx_source(name) {
        if let Some(descriptor) = load_midi_fx_descriptors_from_source(source)
            .into_iter()
            .find(|desc| desc.name.eq_ignore_ascii_case(name))
        {
            return Some(descriptor);
        }
    }

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
        lease: result.lease,
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
        lease: result.lease,
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
                            lease: None,
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
#[path = "lisp_host_tests.rs"]
mod tests;
