use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use eseqlisp::frame as eseq_frame;
use eseqlisp::tui as eseq_tui;
use eseqlisp::vm::Value as EValue;
use eseqlisp::{BufferMode, CompileKind, Editor, EditorConfig, HostCommand, HostEvent, Runtime};
use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::audiograph::{self, LiveGraph, NodeVTable};
use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::scheduled_event::{
    ScheduledEffectParam, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
};
use crate::sequencer::{StepParam, StepSnapshot, Timebase};

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

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
);

// ── Global process function registry ──
// Each track can have up to MAX_CUSTOM_FX custom effects.
// The process fn pointer is stored here, indexed by slot_id = track * MAX_CUSTOM_FX + offset.

use crate::sequencer::MAX_TRACKS;
pub const MAX_CUSTOM_FX: usize = 8;
pub const MAX_MIDI_FX_SLOTS: usize = 4;
const REGISTRY_SIZE: usize = MAX_TRACKS * MAX_CUSTOM_FX;
static DGEN_PROCESS_FNS: [AtomicUsize; REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; REGISTRY_SIZE]
};

fn set_dgen_process_fn(slot_id: usize, f: DGenProcessFn) {
    DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].store(f as usize, Ordering::Release);
}

// ── Node state layout ──
// state[0] = slot_id (f32), where slot_id = track_idx * MAX_CUSTOM_FX + offset
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4..4+N] = DGenLisp read buffer
// state[...]     = DGenLisp write buffer (separate to respect `restrict`)

pub const HEADER_SLOTS: usize = 4;
pub const DGEN_STATE_REDZONE_SLOTS: usize = 256;
const HEADER_CANARY: f32 = f32::from_bits(0x4cd35a1d);

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
    let fn_ptr = DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let _total_memory_slots = *s.add(1) as usize;
        let memory_read = dgen_read_buffer_ptr(s) as *mut c_void;
        let memory_write = dgen_write_buffer_ptr(s, _total_memory_slots) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        process_fn(inp, out, nframes, memory_read, memory_write);
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
///   [4] = num_entries (N)
///   [5..5+2N] = pairs of (index, value)
unsafe extern "C" fn dgenlisp_init(
    state: *mut c_void,
    _sample_rate: c_int,
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

    // Apply sparse index/value pairs into the memory region
    let num_entries = (*src.add(4)) as usize;
    let total_memory_slots = *dst.add(1) as usize;
    let mem = dgen_read_buffer_ptr(dst);
    for i in 0..num_entries {
        let idx = (*src.add(5 + i * 2)) as usize;
        let val = *src.add(5 + i * 2 + 1);
        *mem.add(idx) = val;
    }
    let write_mem = dgen_write_buffer_ptr(dst, total_memory_slots);
    std::ptr::copy_nonoverlapping(mem as *const f32, write_mem, total_memory_slots);
}

fn dgenlisp_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
    }
}

// ── Manifest types ──

#[derive(Clone)]
pub struct DGenManifest {
    pub dylib_path: PathBuf,
    pub total_memory_slots: usize,
    pub params: Vec<DGenParam>,
    pub inputs: Vec<DGenInput>,
    pub modulators: Vec<DGenModulator>,
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
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub unit: Option<String>,
    pub hidden: bool,
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

#[derive(Clone)]
pub struct DGenModDestination {
    pub name: String,
    pub param_cell_id: usize,
    pub source_cell_id: usize,
    pub depth_cell_id: usize,
    pub mode: String,
    pub min: f32,
    pub max: f32,
    pub unit: Option<String>,
    pub depth_min: Option<f32>,
    pub depth_max: Option<f32>,
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
}

#[derive(Clone, Debug)]
pub struct InstrumentRenderReport {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    pub mean_abs: f32,
    pub nonzero_frames: usize,
    pub first_nonzero_frame: Option<usize>,
    pub first_samples: Vec<f32>,
}

pub fn compile_and_load(source: &str, sample_rate: u32) -> Result<CompileResult, String> {
    let json = compile_lisp(source, sample_rate)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult { manifest, lib })
}

// ── Effect library storage ──

const EFFECTS_DIR: &str = "effects";
const INSTRUMENTS_DIR: &str = "instruments";

pub fn save_effect(name: &str, source: &str) -> io::Result<()> {
    let dir = Path::new(EFFECTS_DIR);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.lisp", name));
    std::fs::write(&path, source)
}

pub fn list_saved_effects() -> Vec<String> {
    let dir = Path::new(EFFECTS_DIR);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let path = e.path();
                    if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                        path.file_stem().map(|s| s.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub fn load_effect_source(name: &str) -> io::Result<String> {
    let path = Path::new(EFFECTS_DIR).join(format!("{}.lisp", name));
    std::fs::read_to_string(&path)
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

fn normalize_lisp_source(source: &str) -> String {
    source
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn source_matches_repo_file(source: &str, relative_path: &str) -> bool {
    let repo_path = std::env::current_dir()
        .unwrap_or_default()
        .join(relative_path);
    let Ok(repo_source) = std::fs::read_to_string(repo_path) else {
        return false;
    };
    normalize_lisp_source(source) == normalize_lisp_source(&repo_source)
}

fn should_build_guarded_instrument_dylib(source: &str) -> bool {
    // Temporary debug hook for instrument index-guard dylibs. Leave disabled now
    // that the DGen wrap fix is in, but keep the helper available for future regressions.
    let _ = source;
    return false;
    #[allow(unreachable_code)]
    match std::env::var("TINYSEQ_DGEN_GUARD_INSTRUMENTS") {
        Ok(value) => {
            let value = value.trim();
            if value == "1"
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("yes")
            {
                return true;
            }
        }
        Err(_) => {}
    }
    source_matches_repo_file(source, "instruments/flute.lisp")
}

fn guard_rewrite_memory_index(inner: &str) -> Option<String> {
    let (base, expr_part) = inner.split_once(" + ")?;
    let base = base.trim();
    let expr_part = expr_part.trim();

    if let Some(expr) = expr_part.strip_prefix("(int)") {
        let expr = expr.trim();
        return Some(format!(
            "{base} + dgen_guard_delay_index_f32(({expr}), 88000, {expr:?}, __LINE__)"
        ));
    }

    const FINITE_PREFIX: &str = "(isfinite((int) ";
    const FINITE_MID: &str = ") ? (int) ";
    const FINITE_SUFFIX: &str = " : 0)";
    if expr_part.starts_with(FINITE_PREFIX) && expr_part.ends_with(FINITE_SUFFIX) {
        let tail = &expr_part[FINITE_PREFIX.len()..expr_part.len() - FINITE_SUFFIX.len()];
        let (lhs, rhs) = tail.split_once(FINITE_MID)?;
        let lhs = lhs.trim();
        let rhs = rhs.trim();
        if lhs == rhs {
            return Some(format!(
                "{base} + dgen_guard_delay_index_f32(({lhs}), 88000, {lhs:?}, __LINE__)"
            ));
        }
    }

    None
}

fn rewrite_generated_c_memory_guards(source: &str) -> Result<String, String> {
    let helper = r#"
#include <stdlib.h>

static inline int dgen_guard_delay_index_f32(float idx, int limit, const char *expr, int line) {
  if (!isfinite(idx) || idx < 0.0f || idx >= (float)limit) {
    fprintf(stderr,
            "DGen delay index guard tripped: expr=%s idx=%f limit=%d line=%d\n",
            expr, idx, limit, line);
    abort();
  }
  return (int)idx;
}

"#;
    let injected = if source.contains("dgen_guard_delay_index_f32(") {
        source.to_string()
    } else if source.contains("const int VOICE_COUNT =") {
        source.replacen(
            "const int VOICE_COUNT =",
            &format!("{helper}const int VOICE_COUNT ="),
            1,
        )
    } else {
        return Err("Generated C did not contain VOICE_COUNT anchor for guard injection".into());
    };

    let mut rewritten = String::with_capacity(injected.len() + 1024);
    for line in injected.lines() {
        if !line.contains("memory[") {
            rewritten.push_str(line);
            rewritten.push('\n');
            continue;
        }

        let mut cursor = 0usize;
        let mut changed = String::with_capacity(line.len() + 64);
        while let Some(rel_start) = line[cursor..].find("memory[") {
            let start = cursor + rel_start;
            let open = start + "memory[".len();
            let mut depth = 1i32;
            let mut close = None;
            for (off, ch) in line[open..].char_indices() {
                match ch {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + off);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close_idx) = close else {
                return Err(format!(
                    "Failed to find closing ] in generated C line: {line}"
                ));
            };
            changed.push_str(&line[cursor..open]);
            let inner = &line[open..close_idx];
            if let Some(rewritten_inner) = guard_rewrite_memory_index(inner) {
                changed.push_str(&rewritten_inner);
            } else {
                changed.push_str(inner);
            }
            changed.push(']');
            cursor = close_idx + 1;
        }
        changed.push_str(&line[cursor..]);
        rewritten.push_str(&changed);
        rewritten.push('\n');
    }

    Ok(rewritten)
}

fn compile_guarded_dylib(c_path: &Path, dylib_path: &Path) -> Result<(), String> {
    let output = std::process::Command::new("/usr/bin/clang")
        .args(["-O3", "-fno-fast-math", "-mcpu=native", "-flto=thin"])
        .args(["-fPIC", "-shared"])
        .args(["-framework", "Accelerate"])
        .args(["-std=c11", "-x", "c"])
        .args([
            "-o",
            dylib_path.to_str().ok_or("Invalid dylib output path")?,
        ])
        .arg(c_path)
        .output()
        .map_err(|e| format!("Failed to run clang for guarded dylib: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("Guarded dylib compile failed:\n{stderr}{stdout}"));
    }

    Ok(())
}

fn maybe_rebuild_guarded_instrument_dylib(source: &str, manifest_json: &str) -> Result<(), String> {
    if !should_build_guarded_instrument_dylib(source) {
        return Ok(());
    }

    let manifest: serde_json::Value = serde_json::from_str(manifest_json)
        .map_err(|e| format!("Failed to parse DGen manifest for guard rebuild: {e}"))?;
    let c_source_path = manifest["cSourcePath"]
        .as_str()
        .ok_or("DGen manifest missing cSourcePath for guard rebuild")?;
    let dylib_name = manifest["dylib"]
        .as_str()
        .ok_or("DGen manifest missing dylib for guard rebuild")?;
    let c_source_path = PathBuf::from(c_source_path);
    let dylib_path = c_source_path
        .parent()
        .unwrap_or(output_dir().as_path())
        .join(dylib_name);
    let generated_c = std::fs::read_to_string(&c_source_path)
        .map_err(|e| format!("Failed to read generated C for guard rebuild: {e}"))?;
    let guarded_c = rewrite_generated_c_memory_guards(&generated_c)?;
    let stem = c_source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("dgen_guarded");
    let guarded_c_path = c_source_path.with_file_name(format!("{stem}.guarded.c"));
    std::fs::write(&guarded_c_path, guarded_c)
        .map_err(|e| format!("Failed to write guarded DGen C: {e}"))?;
    compile_guarded_dylib(&guarded_c_path, &dylib_path)
}

pub fn compile_lisp(source: &str, sample_rate: u32) -> Result<String, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    // Unique name per compile so dlopen doesn't return a stale cached handle
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("effect_{}", seq);

    let src_path = dir.join("effect.lisp");
    std::fs::write(&src_path, source).map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp");
    let output = std::process::Command::new(&tool_path)
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", &dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()])
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("{}{}", stderr, stdout));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

// ── Parse manifest ──

pub fn parse_manifest(json: &str) -> Result<DGenManifest, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse manifest: {e}"))?;

    let dir = output_dir();
    let dylib_name = v["dylib"].as_str().unwrap_or("effect.dylib");
    let dylib_path = dir.join(dylib_name);

    let params = v["params"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|p| DGenParam {
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    cell_id: p["cellId"].as_u64().unwrap_or(0) as usize,
                    default: p["default"].as_f64().unwrap_or(0.0) as f32,
                    min: p["min"].as_f64().unwrap_or(0.0) as f32,
                    max: p["max"].as_f64().unwrap_or(1.0) as f32,
                    unit: p["unit"].as_str().map(|s| s.to_string()),
                    hidden: p["hidden"].as_bool().unwrap_or(false),
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

    let mod_destinations = v["modDestinations"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|m| DGenModDestination {
                    name: m["name"].as_str().unwrap_or("").to_string(),
                    param_cell_id: m["paramCellId"].as_u64().unwrap_or(0) as usize,
                    source_cell_id: m["sourceCellId"].as_u64().unwrap_or(0) as usize,
                    depth_cell_id: m["depthCellId"].as_u64().unwrap_or(0) as usize,
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
                })
                .collect()
        })
        .unwrap_or_default();

    let voice_cell_id = v["voiceCellId"].as_u64().map(|id| id as usize);

    Ok(DGenManifest {
        dylib_path,
        total_memory_slots: v["totalMemorySlots"].as_u64().unwrap_or(256) as usize,
        params,
        inputs,
        modulators,
        mod_destinations,
        n_inputs,
        n_outputs,
        tensors,
        tensor_init_data,
        voice_cell_id,
    })
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
/// [slot_id, total_memory_slots, canary, declared_input_count, num_entries, idx0, val0, ...]
/// The engine zeroes state; init only needs to set non-zero values.
fn build_init_message(slot_id: usize, manifest: &DGenManifest) -> Vec<f32> {
    // Collect all non-zero index/value pairs
    let mut entries: Vec<(usize, f32)> = Vec::new();

    for param in &manifest.params {
        if param.cell_id < manifest.total_memory_slots && param.default != 0.0 {
            for lane in 0..4 {
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

    // Header (5) + pairs (2 * N)
    let mut msg = Vec::with_capacity(5 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
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

unsafe fn connect_effect_chain(
    lg: *mut LiveGraph,
    predecessor_id: i32,
    predecessor_outputs: usize,
    effect_id: i32,
    effect_inputs: usize,
    effect_outputs: usize,
    successor_id: i32,
    successor_inputs: usize,
) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            audiograph::graph_disconnect(lg, predecessor_id, src_port, successor_id, dst_port);
        }
    }

    if effect_inputs <= 1 {
        let pred_channels = predecessor_outputs.max(1).min(2);
        for src_port in 0..pred_channels {
            let _ = audiograph::graph_connect(lg, predecessor_id, src_port as i32, effect_id, 0);
        }
    } else {
        let pred_channels = predecessor_outputs.max(1).min(2);
        for ch in 0..pred_channels.min(effect_inputs).min(2) {
            let _ = audiograph::graph_connect(lg, predecessor_id, ch as i32, effect_id, ch as i32);
        }
    }

    if effect_outputs <= 1 {
        let succ_channels = successor_inputs.max(1).min(2);
        for dst_port in 0..succ_channels {
            let _ = audiograph::graph_connect(lg, effect_id, 0, successor_id, dst_port as i32);
        }
    } else {
        let succ_channels = successor_inputs.max(1).min(2);
        for ch in 0..succ_channels.min(effect_outputs).min(2) {
            let _ = audiograph::graph_connect(lg, effect_id, ch as i32, successor_id, ch as i32);
        }
    }
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
) -> Result<i32, String> {
    // Remove old effect if present
    if let Some(old_id) = existing_effect {
        remove_effect_from_chain(lg, old_id, predecessor_id, successor_id);
    }

    // Register process function
    set_dgen_process_fn(slot_id, lib.process_fn);

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

    connect_effect_chain(
        lg,
        predecessor_id,
        predecessor_outputs,
        node_id,
        manifest.n_inputs,
        manifest.n_outputs,
        successor_id,
        successor_inputs,
    );

    Ok(node_id)
}

// ── Full interactive editor-compile-load flow ──

pub const EFFECT_TEMPLATE: &str = r#"; DGenLisp stereo effect
;
; Params: (def name (param name @min 0 @max 1 @default 0.5))
; Delay:  (def h (history N)), (read-history h delay_samples), (write-history h sample)
; Math:   +, -, *, /, tanh, clamp, min, max, mix
; Filters: (onepole input coeff)

(def input (in 1 @name signal))
(def mix-amt (param mix @min 0 @max 1 @default 0.5))

; -- Your processing here --
(def processed input)

; -- Stereo output --
(out (mix input processed mix-amt) 1 @name Left)
(out (mix input processed mix-amt) 2 @name Right)
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
                                    )
                                } {
                                    Ok(node_id) => {
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
                                            node_id,
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
struct InstrumentPresetBank {
    version: u32,
    engine_name: String,
    source_file: String,
    presets: Vec<InstrumentPreset>,
}

fn resolve_instrument_storage_path(name: &str, extension: &str) -> io::Result<PathBuf> {
    fn collect_matches(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_matches(&path, file_name, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
                out.push(path);
            }
        }
    }

    let root = Path::new(INSTRUMENTS_DIR);
    let folder_name = name.trim_end_matches('/');
    if extension == "lisp" && name.ends_with('/') {
        let dsp = root.join(folder_name).join("dsp.lisp");
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

    let basename = Path::new(folder_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(folder_name);
    let file_name = format!("{basename}.{extension}");
    let mut matches = Vec::new();
    collect_matches(root, &file_name, &mut matches);
    matches.sort_by_key(|path| path.to_string_lossy().to_lowercase());

    match matches.len() {
        0 => Ok(exact),
        1 => Ok(matches.remove(0)),
        _ => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Ambiguous instrument '{name}': found multiple {file_name} files under {INSTRUMENTS_DIR}"
            ),
        )),
    }
}

pub fn instrument_source_path(name: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path(name, "lisp")
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
        CompileKind::Effect => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string()),
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
    const INIT: AtomicUsize = AtomicUsize::new(MAX_VOICES);
    [INIT; MAX_TRACKS]
};

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
        MAX_VOICES
    }
}

pub fn reset_dgen_engine_enabled_voices(engine_id: usize) {
    set_dgen_engine_enabled_voices(engine_id, 1);
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
        process_fn(inp, out, nframes, memory_read, memory_write);
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
            for lane in 0..4 {
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

    let mut msg = Vec::with_capacity(5 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Instrument storage ──

pub fn save_instrument(name: &str, source: &str) -> io::Result<()> {
    let path = resolve_instrument_storage_path(name, "lisp")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
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
; Assumes 44.1 kHz for time-based helpers.

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
  (def dt (clip (/ freq 44100.0) 0.000001 0.5))
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

(defmacro adsr (gate_sig trigger_sig attack_ms decay_ms sustain release_ms)
  (make-history env)
  (make-history gate_hist)
  (make-history stage_hist)

  ; Retriggers first fade any leftover voice history to silence over a
  ; short de-click window, then start a linear attack from near zero.
  ; Decay/release are one-pole curves scaled to settle near the target
  ; over the requested number of milliseconds.
  (def sr 44100.0)
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
    let source_with_preamble = format!("{INSTRUMENT_PREAMBLE}\n\n{source}");
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
        return Err(format!("{}{}", stderr, stdout));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    maybe_rebuild_guarded_instrument_dylib(source, &stdout)?;
    Ok(stdout)
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
    let entry_count = init_msg.get(4).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[5 + i * 2] as usize;
        let value = init_msg[5 + i * 2 + 1];
        if idx < total_slots {
            memory_read[idx] = value;
        }
    }
    memory_write.copy_from_slice(&memory_read);

    for (name, value) in &options.param_overrides {
        let param = manifest
            .params
            .iter()
            .find(|param| param.name == *name)
            .ok_or_else(|| format!("unknown instrument parameter '{name}'"))?;
        if param.cell_id >= total_slots {
            return Err(format!(
                "parameter '{}' cell {} is outside memory size {}",
                param.name, param.cell_id, total_slots
            ));
        }
        for lane in 0..4 {
            let idx = param.cell_id + lane;
            if idx < total_slots {
                memory_read[idx] = *value;
                memory_write[idx] = *value;
            }
        }
    }

    let pitch_hz = 440.0 * 2f32.powf((options.midi_note - 69.0) / 12.0);
    let n_inputs = manifest.n_inputs.max(4);
    let n_outputs = manifest.n_outputs.max(1);
    let mut rendered = Vec::with_capacity(options.frames);
    let mut frames_done = 0usize;

    while frames_done < options.frames {
        let block = options.block_size.min(options.frames - frames_done);
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
            );
        }
        memory_read.copy_from_slice(&memory_write);
        rendered.extend_from_slice(&output_buffers[0]);
        frames_done += block;
    }

    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut nonzero_frames = 0usize;
    let mut first_nonzero_frame = None;
    for (idx, sample) in rendered.iter().enumerate() {
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

    Ok(InstrumentRenderReport {
        frames: rendered.len(),
        peak,
        rms,
        mean_abs,
        nonzero_frames,
        first_nonzero_frame,
        first_samples: rendered.into_iter().take(32).collect(),
    })
}

// ── Instrument editor flow ──

pub const INSTRUMENT_TEMPLATE: &str = r#"; DGenLisp instrument
;
; Params:  (param name @default 1.0 @min 0 @max 10)
; Modulatable: add @mod true @mod-mode additive
;   then use (mod name) to read the modulated value
; Envelope: (adsr gate trigger attack_ms decay_ms sustain release_ms)
; Oscillators: (sin expr), (phasor freq_hz), (noise)
; Math: +, -, *, /, tanh, clamp, min, max
; Constants: twopi, samplerate

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

; -- Parameters --
(param attack  @default 5    @min 0   @max 1000 @unit ms)
(param release @default 200  @min 10  @max 5000 @unit ms)
(param gain    @default 0.5  @min 0   @max 1    @mod true @mod-mode additive)

; -- Envelope --
(def env (adsr gate trigger attack 100 1 release))

; -- Oscillator --
(def osc (sin (* (phasor pitch) twopi)))

; -- Output --
(out (* osc env velocity (mod gain)) 1 @name audio)
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
        CompileKind::Effect => {
            Path::new(EFFECTS_DIR).join(format!("{}.lisp", existing_name.unwrap_or("untitled")))
        }
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

pub struct ScratchControlRuntime {
    runtime: Runtime,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
    accumulators: SharedRegisteredAccumulators,
    midi_fx: SharedRegisteredMidiFx,
    pending_midi_fx_params: SharedPendingMidiFxParams,
    midi_fx_state: SharedMidiFxState,
    accumulator_eval: SharedAccumulatorEvalContext,
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
        );
        let mut this = Self {
            runtime,
            context,
            metadata,
            accumulators,
            midi_fx,
            pending_midi_fx_params,
            midi_fx_state,
            accumulator_eval,
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
        )
    }

    pub(crate) fn from_parts(
        runtime: Runtime,
        context: SharedSequencerEvalContext,
        metadata: SharedSequencerNativeMetadata,
        accumulators: SharedRegisteredAccumulators,
        midi_fx: SharedRegisteredMidiFx,
        pending_midi_fx_params: SharedPendingMidiFxParams,
        midi_fx_state: SharedMidiFxState,
        accumulator_eval: SharedAccumulatorEvalContext,
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
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
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
    );
}

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
) {
    let current_track =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.track).unwrap_or(0);
    let current_step =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.cursor_step).unwrap_or(0);

    let _ = install_runtime_globals(runtime, &context, &metadata, &[]);

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
            let params = pending_params_for_register
                .lock()
                .map_err(|_| "failed to lock pending MIDI FX params".to_string())?
                .drain(..)
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
            slot.plocks.set(step_idx, param_idx, param_desc.clamp(value));
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
            slot.plocks.set(step_idx, param_idx, value);
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
            slot.plocks.set(step_idx, param_idx, value);
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
            slot.plocks.set(step_idx, param_idx, value);
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
            slot.plocks.set(step_idx, param_idx, value);
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
        .map(|_| EffectDescriptor::builtin_delay())
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
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync => {}
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
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync => {}
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
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync => {}
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
) -> Result<(ScheduledInstrumentParamTarget, u64), String> {
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("instrument param index out of range".to_string());
    }
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    Ok(if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
        )
    } else {
        (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
    })
}

fn current_instrument_param_raw(
    eval: &AccumulatorEvalContext,
    param_idx: usize,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<f32, String> {
    let (target, idx) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
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
    let (target, idx) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
    let value = desc.clamp(value);
    if let Some(existing) = eval
        .instrument_params
        .iter_mut()
        .find(|param| param.target == target && param.idx == idx)
    {
        existing.value = value;
    } else {
        eval.instrument_params
            .push(ScheduledInstrumentParam { target, idx, value });
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
                _ => Err("unknown step param".to_string()),
            }
        }
        _ => Err("expected step param keyword/string".to_string()),
    }
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
        host_control: None,
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
        "chord".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord
                .into_iter()
                .map(|note| EValue::Number(note as f64))
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
    let init_src = std::fs::read_to_string("../eseqlisp/init.lisp")
        .or_else(|_| std::fs::read_to_string("init.lisp"))
        .unwrap_or_default();
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
    ) = control_runtime.into_parts();
    let init_src = std::fs::read_to_string("../eseqlisp/init.lisp")
        .or_else(|_| std::fs::read_to_string("init.lisp"))
        .unwrap_or_default();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
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
    if runtime.eval(&source).is_err() {
        return Vec::new();
    }
    runtime.midi_fx_descriptors()
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
        compile_instrument_with_asset_base, fallback_effect_descriptors,
        fallback_instrument_descriptors, new_eval_context, parse_manifest,
        register_sequencer_natives, scratch_runtime_with_fallbacks, shared_native_metadata,
        AccumulatorNoteSpan, DGenParam, ScratchControlRuntime,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
    use crate::scheduled_event::{
        ScheduledEffectParam, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    };
    use crate::sequencer::{default_empty_effect_chain, SequencerState, StepParam};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::vm::Value;
    use eseqlisp::{BufferMode, Editor, EditorConfig, Runtime};
    use std::sync::atomic::Ordering;

    #[test]
    fn folder_instrument_dsp_path_maps_to_instrument_name() {
        let path = std::path::Path::new("instruments/emulations/monomachine-fmplus/dsp.lisp");
        assert_eq!(
            super::instrument_name_from_source_path(path).as_deref(),
            Some("emulations/monomachine-fmplus/")
        );
        assert_eq!(
            super::source_name_from_path(&eseqlisp::CompileKind::Instrument, path).as_deref(),
            Some("emulations/monomachine-fmplus/")
        );
    }
    use std::sync::Arc;

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
        state.pattern.effect_chains[0][0].plocks.set(2, 0, 0.25);

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
        let mut runtime = Runtime::new();
        let effect_descriptors = fallback_effect_descriptors(1);
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
                fallback_effect_descriptors(1),
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
                fallback_effect_descriptors(1),
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
                assert_eq!(names, vec!["enabled", "mode", "cutoff", "resonance"]);
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
                fallback_effect_descriptors(1),
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
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
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
                default: 0.0,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
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
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
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
        let init_src = std::fs::read_to_string("../eseqlisp/init.lisp")
            .or_else(|_| std::fs::read_to_string("init.lisp"))
            .unwrap_or_default();
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
            fallback_effect_descriptors(1),
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
            fallback_effect_descriptors(1),
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
            fallback_effect_descriptors(1),
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
        assert_eq!(slot.num_params.load(Ordering::Relaxed), 5);
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
        let init_src = std::fs::read_to_string("../eseqlisp/init.lisp")
            .or_else(|_| std::fs::read_to_string("init.lisp"))
            .unwrap_or_default();
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
