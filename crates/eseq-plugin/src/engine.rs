//! The instrument engine one plugin instance runs: N DGen voices, note
//! allocation, the per-sample host signals (gate/pitch/velocity/trigger/clock)
//! and param writes into voice state. Nothing here allocates after
//! [`Engine::new`]; `process` is real-time safe.

use std::ffi::{CString, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::payload::{DspLayout, HostSignal, ParamSpec, Payload};

const DGEN_ABI_VERSION_V1: u32 = 1;

#[repr(C)]
pub struct DGenProcessContextV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub sample_rate: f32,
    pub reserved: u32,
}

#[repr(C)]
pub struct DGenHostServicesV1 {
    _private: [u8; 0],
}

pub type DGenProcessFn = unsafe extern "C" fn(
    *const *const f32,
    *const *mut f32,
    u32,
    *mut c_void,
    *const DGenProcessContextV1,
    *const DGenHostServicesV1,
);

unsafe extern "C" {
    fn eseq_dgen_host_services_v1() -> *const DGenHostServicesV1;
}

/// A dlopen'ed DGen library. Kept open for the life of the process: instances
/// come and go, and the image's scratch statics are shared by all of them.
pub struct DspLib {
    pub process: DGenProcessFn,
}

impl DspLib {
    pub fn open(path: &Path) -> Result<Self, String> {
        let c_path = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| format!("bad library path {}", path.display()))?;
        unsafe {
            let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if handle.is_null() {
                let err = std::ffi::CStr::from_ptr(libc::dlerror()).to_string_lossy().into_owned();
                return Err(format!("dlopen {}: {err}", path.display()));
            }
            let sym = libc::dlsym(handle, c"dgen_process_v1".as_ptr());
            if sym.is_null() {
                return Err(format!("{} exports no dgen_process_v1", path.display()));
            }
            Ok(Self { process: std::mem::transmute::<*mut c_void, DGenProcessFn>(sym) })
        }
    }
}

/// The voice-scratch slices of the shared DGen image. Generated code indexes
/// its file-scope scratch statics by the voice cell, so two voices processing
/// concurrently (two instances on two DAW threads) must hold distinct slices.
/// Claimed at activate, released at deactivate; never touched by `process`.
pub struct SlotPool {
    used: Mutex<Vec<bool>>,
}

impl SlotPool {
    pub fn global(capacity: usize) -> &'static SlotPool {
        static POOL: OnceLock<SlotPool> = OnceLock::new();
        POOL.get_or_init(|| SlotPool { used: Mutex::new(vec![false; capacity.max(1)]) })
    }

    pub fn claim(&self, wanted: usize) -> Vec<usize> {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        let mut slots = Vec::with_capacity(wanted);
        for (slot, taken) in used.iter_mut().enumerate() {
            if slots.len() == wanted {
                break;
            }
            if !*taken {
                *taken = true;
                slots.push(slot);
            }
        }
        slots
    }

    pub fn release(&self, slots: &[usize]) {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        for &slot in slots {
            if let Some(taken) = used.get_mut(slot) {
                *taken = false;
            }
        }
    }
}

/// Host-visible param values, shared by the audio thread, the host's main
/// thread and the editor. Stored units (what the DSP cell holds), as f32 bits.
pub struct ParamStore {
    values: Vec<AtomicU32>,
}

impl ParamStore {
    pub fn new(defaults: impl IntoIterator<Item = f32>) -> Self {
        Self { values: defaults.into_iter().map(|v| AtomicU32::new(v.to_bits())).collect() }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    pub fn set(&self, index: usize, value: f32) {
        self.values[index].store(value.to_bits(), Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum NoteEvent {
    On { key: i16, channel: i16, note_id: i32, velocity: f32 },
    Off { key: i16, channel: i16, note_id: i32 },
    Choke { key: i16, channel: i16 },
    AllOff,
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Note(NoteEvent),
    Param { index: usize, value: f32 },
}

impl Event {
    /// Dropping this event would leave a note hanging.
    pub fn ends_a_note(&self) -> bool {
        matches!(self, Event::Note(NoteEvent::Off { .. } | NoteEvent::Choke { .. } | NoteEvent::AllOff))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimedEvent {
    pub frame: u32,
    pub event: Event,
}

/// Bar position for the `clock` input: phase 0..1 through the current bar and
/// its per-sample increment.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub bar_phase: f64,
    pub bar_inc: f64,
}

impl Transport {
    pub fn free_running(sample_rate: f32) -> Self {
        // 120 BPM in 4/4 when the host provides no transport.
        Self { bar_phase: 0.0, bar_inc: 2.0 / 4.0 / sample_rate.max(1.0) as f64 }
    }
}

struct Voice {
    memory: Box<[f32]>,
    slot: usize,
    key: i16,
    channel: i16,
    note_id: i32,
    gate: bool,
    trigger_pending: bool,
    pitch_hz: f32,
    velocity: f32,
    /// Sounding: gate held, or released and the `@amp` flag still on.
    active: bool,
    /// Frames rendered since release with no audible output (no-@amp fallback).
    quiet_frames: u32,
    started: u64,
}

pub struct Engine {
    process: DGenProcessFn,
    host_services: *const DGenHostServicesV1,
    sample_rate: f32,
    layout: DspLayout,
    initial_state: Arc<[f32]>,
    voices: Vec<Voice>,
    slots: Vec<usize>,
    slot_pool: &'static SlotPool,
    params: Arc<ParamStore>,
    specs: Vec<ParamSpec>,
    applied: Vec<f32>,
    params_dirty: bool,
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    input_ptrs: Vec<*const f32>,
    output_ptrs: Vec<*mut f32>,
    /// The `clock` ramp of the current span, shared by every voice.
    clock: Vec<f32>,
    note_counter: u64,
}

// The raw pointers are the static host-services table and pointers into this
// engine's own buffers.
unsafe impl Send for Engine {}

/// Seconds of silence after release before a voice without an `@amp` output
/// is retired.
const QUIET_RETIRE_SECONDS: f32 = 0.5;
const QUIET_THRESHOLD: f32 = 1.0e-5;

impl Engine {
    pub fn new(payload: &Payload, lib: &DspLib, params: Arc<ParamStore>, sample_rate: f32) -> Result<Self, String> {
        let layout = payload.spec.dsp.layout.clone();
        let outputs_ok = [Some(layout.audio_left), Some(layout.audio_right), layout.amp_channel]
            .into_iter()
            .flatten()
            .all(|channel| channel < layout.n_outputs);
        if !outputs_ok || layout.routes.iter().any(|r| r.channel >= layout.n_inputs) {
            return Err("the DSP layout names channels outside its inputs/outputs".into());
        }
        for spec in &payload.spec.params {
            if spec.cell + spec.span > layout.total_memory_slots {
                return Err(format!("param '{}' cells out of range", spec.name));
            }
        }
        let slot_pool = SlotPool::global(payload.spec.dsp.voice_slots);
        let slots = slot_pool.claim(payload.spec.polyphony.max(1));
        if slots.is_empty() {
            return Err("every DGen voice slot of this plugin is in use".into());
        }
        let initial_state: Arc<[f32]> = payload.initial_state.clone().into();
        let voices = slots
            .iter()
            .map(|&slot| {
                let mut voice = Voice {
                    memory: initial_state.to_vec().into_boxed_slice(),
                    slot,
                    key: -1,
                    channel: -1,
                    note_id: -1,
                    gate: false,
                    trigger_pending: false,
                    pitch_hz: 440.0,
                    velocity: 0.0,
                    active: false,
                    quiet_frames: 0,
                    started: 0,
                };
                if let Some(cell) = layout.voice_cell {
                    voice.memory[cell] = slot as f32;
                }
                voice
            })
            .collect();
        let max_frames = layout.max_frames.clamp(1, 4096);
        let mut inputs = vec![vec![0.0f32; max_frames]; layout.n_inputs];
        let mut outputs = vec![vec![0.0f32; max_frames]; layout.n_outputs.max(1)];
        let input_ptrs = inputs.iter_mut().map(|b| b.as_ptr()).collect();
        let output_ptrs = outputs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let specs = payload.spec.params.clone();
        let mut engine = Self {
            process: lib.process,
            host_services: unsafe { eseq_dgen_host_services_v1() },
            sample_rate,
            layout,
            initial_state,
            voices,
            slots,
            slot_pool,
            params,
            applied: vec![f32::NAN; specs.len()],
            specs,
            params_dirty: true,
            inputs,
            outputs,
            input_ptrs,
            output_ptrs,
            clock: vec![0.0; max_frames],
            note_counter: 0,
        };
        engine.sync_params();
        Ok(engine)
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Back to the just-activated state: every voice's DSP state restored
    /// (filters, envelopes, delay lines), no note sounding. Current param
    /// values are re-applied.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.memory.copy_from_slice(&self.initial_state);
            if let Some(cell) = self.layout.voice_cell {
                voice.memory[cell] = voice.slot as f32;
            }
            voice.gate = false;
            voice.active = false;
            voice.trigger_pending = false;
            voice.key = -1;
            voice.note_id = -1;
        }
        self.applied.fill(f32::NAN);
        self.params_dirty = true;
        self.sync_params();
    }

    /// Write every param whose store value moved since the last write into
    /// all voices (idle ones too, so a new note starts from current values).
    /// Runs at block start and after a param event within the block.
    fn sync_params(&mut self) {
        if !std::mem::take(&mut self.params_dirty) {
            return;
        }
        for index in 0..self.specs.len() {
            let value = self.params.get(index);
            if value.to_bits() == self.applied[index].to_bits() {
                continue;
            }
            self.applied[index] = value;
            let spec = &self.specs[index];
            let value = spec.clamp(value);
            for voice in &mut self.voices {
                voice.memory[spec.cell..spec.cell + spec.span].fill(value);
            }
        }
    }

    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Param { index, value } => {
                if index < self.params.len() {
                    self.params.set(index, value);
                    self.params_dirty = true;
                }
            }
            Event::Note(NoteEvent::On { key, channel, note_id, velocity }) => {
                let voice = self.pick_voice(key, channel);
                self.note_counter += 1;
                let v = &mut self.voices[voice];
                v.key = key;
                v.channel = channel;
                v.note_id = note_id;
                v.gate = true;
                v.trigger_pending = true;
                v.pitch_hz = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
                v.velocity = velocity.clamp(0.0, 1.0);
                v.active = true;
                v.quiet_frames = 0;
                v.started = self.note_counter;
            }
            Event::Note(NoteEvent::Off { key, channel, note_id }) => {
                for v in &mut self.voices {
                    let id_match = note_id >= 0 && v.note_id == note_id;
                    let key_match = (key < 0 || v.key == key) && (channel < 0 || v.channel == channel);
                    if v.gate && (id_match || (note_id < 0 && key_match)) {
                        v.gate = false;
                        v.quiet_frames = 0;
                    }
                }
            }
            Event::Note(NoteEvent::Choke { key, channel }) => {
                for v in &mut self.voices {
                    if (key < 0 || v.key == key) && (channel < 0 || v.channel == channel) {
                        v.gate = false;
                        v.active = false;
                    }
                }
            }
            Event::Note(NoteEvent::AllOff) => {
                for v in &mut self.voices {
                    v.gate = false;
                }
            }
        }
    }

    /// A free voice, else the same key's voice (retrigger in place), else the
    /// oldest released voice, else the oldest voice.
    fn pick_voice(&self, key: i16, channel: i16) -> usize {
        if let Some(i) = self.voices.iter().position(|v| v.active && v.key == key && v.channel == channel) {
            return i;
        }
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        let oldest = |gated: bool| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.gate == gated)
                .min_by_key(|(_, v)| v.started)
                .map(|(i, _)| i)
        };
        oldest(false).or_else(|| oldest(true)).unwrap_or(0)
    }

    /// Render `frames` stereo frames, applying `events` (sorted by frame) at
    /// their sample offsets.
    pub fn process(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        events: &[TimedEvent],
        transport: &mut Transport,
    ) {
        let frames = out_l.len().min(out_r.len());
        out_l[..frames].fill(0.0);
        out_r[..frames].fill(0.0);
        // The editor and the host's main thread write the store directly
        // (no event reaches the engine for those): look once per block.
        self.params_dirty = true;
        let max_frames = self.clock.len();
        let mut next_event = 0;
        let mut done = 0;
        while done < frames {
            while next_event < events.len() && events[next_event].frame as usize <= done {
                self.apply_event(events[next_event].event);
                next_event += 1;
            }
            let until = events
                .get(next_event)
                .map_or(frames, |e| (e.frame as usize).clamp(done + 1, frames));
            let span = (until - done).min(max_frames);
            self.sync_params();
            self.render_span(&mut out_l[done..done + span], &mut out_r[done..done + span], transport);
            done += span;
        }
        // Events stamped at or past the block end still apply.
        while next_event < events.len() {
            self.apply_event(events[next_event].event);
            next_event += 1;
        }
    }

    fn render_span(&mut self, out_l: &mut [f32], out_r: &mut [f32], transport: &mut Transport) {
        let n = out_l.len();
        let context = DGenProcessContextV1 {
            abi_version: DGEN_ABI_VERSION_V1,
            struct_size: std::mem::size_of::<DGenProcessContextV1>() as u32,
            sample_rate: self.sample_rate,
            reserved: 0,
        };
        let mut phase = transport.bar_phase;
        for sample in &mut self.clock[..n] {
            *sample = phase as f32;
            phase = (phase + transport.bar_inc).fract();
        }
        transport.bar_phase = phase;
        let retire_after = (QUIET_RETIRE_SECONDS * self.sample_rate) as u32;
        let (audio_left, audio_right) = (self.layout.audio_left, self.layout.audio_right);
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            for route in &self.layout.routes {
                let buffer = &mut self.inputs[route.channel][..n];
                match route.signal {
                    HostSignal::Gate => buffer.fill(if voice.gate { 1.0 } else { 0.0 }),
                    HostSignal::Pitch => buffer.fill(voice.pitch_hz),
                    HostSignal::Velocity => buffer.fill(voice.velocity),
                    HostSignal::Trigger | HostSignal::NoteOn => {
                        buffer.fill(0.0);
                        if voice.trigger_pending {
                            buffer[0] = 1.0;
                        }
                    }
                    HostSignal::ClockPhase => buffer.copy_from_slice(&self.clock[..n]),
                    HostSignal::ClockInc => buffer.fill(transport.bar_inc as f32),
                    // Expression inputs eseq's sequencer drives; a plugin
                    // note has none yet (MPE is a follow-up).
                    _ => buffer.fill(0.0),
                }
            }
            voice.trigger_pending = false;
            for buffer in &mut self.outputs {
                buffer[..n].fill(0.0);
            }
            unsafe {
                (self.process)(
                    self.input_ptrs.as_ptr(),
                    self.output_ptrs.as_ptr(),
                    n as u32,
                    voice.memory.as_mut_ptr() as *mut c_void,
                    &context,
                    self.host_services,
                );
            }
            let left = &self.outputs[audio_left][..n];
            let right = &self.outputs[audio_right][..n];
            let mut peak = 0.0f32;
            for i in 0..n {
                let (l, r) = (left[i], right[i]);
                // A NaN from one voice must not poison the host's bus.
                if l.is_finite() && r.is_finite() {
                    out_l[i] += l;
                    out_r[i] += r;
                    peak = peak.max(l.abs()).max(r.abs());
                }
            }
            if !voice.gate {
                match self.layout.amp_channel {
                    Some(channel) => {
                        if self.outputs[channel][..n].iter().all(|&a| a == 0.0) {
                            voice.active = false;
                        }
                    }
                    None => {
                        if peak < QUIET_THRESHOLD {
                            voice.quiet_frames += n as u32;
                            if voice.quiet_frames >= retire_after {
                                voice.active = false;
                            }
                        } else {
                            voice.quiet_frames = 0;
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.slot_pool.release(&self.slots);
    }
}
