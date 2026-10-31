//! CLAP entry, factory and plugin. Everything host-facing lives here; the
//! audio work is `engine`, the panel is `editor`.
//!
//! Threading follows CLAP: `activate`/`deactivate`/`reset`/`process` and
//! `params.flush` never overlap, so the audio state they share sits in an
//! [`UnsafeCell`] only they touch; everything else is reached through `&`.

use std::cell::UnsafeCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR;
use clap_sys::host::clap_host;
use clap_sys::id::{CLAP_INVALID_ID, clap_id};
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

use crate::engine::{DspLib, Engine, Event, NoteEvent, ParamStore, TimedEvent, Transport};
use crate::payload::{ParamSpec, Payload, param_id};

/// Events one process call holds without allocating; note-ending events
/// still get in past it (a dropped note-off is a stuck note).
const EVENT_CAPACITY: usize = 4096;

/// Everything loaded once per bundle: the payload, the dlopen'ed DSP and the
/// CLAP descriptor strings.
pub(crate) struct Shell {
    pub payload: Payload,
    lib: DspLib,
    descriptor: clap_plugin_descriptor,
    _strings: Vec<CString>,
    _features: Vec<*const c_char>,
    pub param_ids: Vec<clap_id>,
    /// (id, index) sorted by id.
    param_lookup: Vec<(clap_id, usize)>,
}

unsafe impl Send for Shell {}
unsafe impl Sync for Shell {}

static SHELL: Mutex<Option<Arc<Shell>>> = Mutex::new(None);

fn shell() -> Option<Arc<Shell>> {
    SHELL.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn log(message: &str) {
    eprintln!("[eseq-plugin] {message}");
}

impl Shell {
    fn load(bundle: &Path) -> Result<Self, String> {
        let payload = Payload::load(&Payload::dir_for_bundle(bundle))?;
        let lib = DspLib::open(&payload.library_path())?;
        let param_ids: Vec<clap_id> = payload.spec.params.iter().map(|p| param_id(&p.name)).collect();
        let mut param_lookup: Vec<(clap_id, usize)> = param_ids.iter().copied().zip(0..).collect();
        param_lookup.sort_unstable();
        let spec = &payload.spec;
        let cstring = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
        let strings = vec![
            cstring(&spec.id),
            cstring(&spec.name),
            cstring(&spec.vendor),
            cstring(&spec.url),
            cstring(&spec.version),
            cstring(&spec.description),
        ];
        let feature_strings: Vec<CString> = spec.features.iter().map(|f| cstring(f)).collect();
        let mut features: Vec<*const c_char> = feature_strings.iter().map(|f| f.as_ptr()).collect();
        features.push(std::ptr::null());
        let descriptor = clap_plugin_descriptor {
            clap_version: CLAP_VERSION,
            id: strings[0].as_ptr(),
            name: strings[1].as_ptr(),
            vendor: strings[2].as_ptr(),
            url: strings[3].as_ptr(),
            manual_url: strings[3].as_ptr(),
            support_url: strings[3].as_ptr(),
            version: strings[4].as_ptr(),
            description: strings[5].as_ptr(),
            features: features.as_ptr(),
        };
        let mut all_strings = strings;
        all_strings.extend(feature_strings);
        Ok(Self {
            payload,
            lib,
            descriptor,
            _strings: all_strings,
            _features: features,
            param_ids,
            param_lookup,
        })
    }

    fn param_index(&self, id: clap_id) -> Option<usize> {
        let at = self.param_lookup.binary_search_by_key(&id, |&(id, _)| id).ok()?;
        Some(self.param_lookup[at].1)
    }

    fn param(&self, id: clap_id) -> Option<(usize, &ParamSpec)> {
        let index = self.param_index(id)?;
        Some((index, &self.payload.spec.params[index]))
    }
}

/// The bundle this image lives in, from the image path: `X.clap/Contents/MacOS/X`.
fn bundle_of_this_image() -> Option<PathBuf> {
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(bundle_of_this_image as *const c_void, &mut info) == 0 || info.dli_fname.is_null() {
            return None;
        }
        let binary = PathBuf::from(CStr::from_ptr(info.dli_fname).to_string_lossy().into_owned());
        Some(binary.parent()?.parent()?.parent()?.to_path_buf())
    }
}

// ── entry ───────────────────────────────────────────────────────────────

unsafe extern "C" fn entry_init(plugin_path: *const c_char) -> bool {
    let mut candidates = Vec::new();
    if !plugin_path.is_null() {
        candidates.push(PathBuf::from(unsafe { CStr::from_ptr(plugin_path) }.to_string_lossy().into_owned()));
    }
    candidates.extend(bundle_of_this_image());
    let mut guard = SHELL.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_some() {
        return true;
    }
    for bundle in candidates {
        match Shell::load(&bundle) {
            Ok(shell) => {
                *guard = Some(Arc::new(shell));
                return true;
            }
            Err(err) => log(&format!("no payload at {}: {err}", bundle.display())),
        }
    }
    false
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    if factory_id.is_null() || unsafe { CStr::from_ptr(factory_id) } != CLAP_PLUGIN_FACTORY_ID {
        return std::ptr::null();
    }
    &FACTORY as *const clap_plugin_factory as *const c_void
}

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

// ── factory ─────────────────────────────────────────────────────────────

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn factory_count(_: *const clap_plugin_factory) -> u32 {
    shell().is_some() as u32
}

unsafe extern "C" fn factory_descriptor(_: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    match shell() {
        // The Arc in SHELL lives until the image unloads.
        Some(shell) if index == 0 => &shell.descriptor as *const _,
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(
    _: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    let Some(shell) = shell() else { return std::ptr::null() };
    if plugin_id.is_null() || unsafe { CStr::from_ptr(plugin_id) }.to_bytes() != shell.payload.spec.id.as_bytes() {
        return std::ptr::null();
    }
    let raw = Box::into_raw(Instance::new(shell, host));
    unsafe {
        (*raw).clap.plugin_data = raw as *mut c_void;
        &(*raw).clap
    }
}

// ── instance ────────────────────────────────────────────────────────────

/// A param edit made in the editor, forwarded to the host as CLAP events.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GuiEdit {
    Begin(usize),
    Value(usize, f32),
    End(usize),
}

/// Shared between the instance and its editor.
pub(crate) struct Shared {
    pub shell: Arc<Shell>,
    pub params: Arc<ParamStore>,
    gui_edits: Mutex<Vec<GuiEdit>>,
    host: *const clap_host,
    host_params: Option<&'static clap_host_params>,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    /// Queue editor edits for the host (main thread). The editor has already
    /// written the values into the store. A pending value for the same param
    /// is replaced, so a fast drag never piles up between audio blocks.
    pub fn send_edits(&self, edits: impl IntoIterator<Item = GuiEdit>) {
        {
            let mut queue = self.gui_edits.lock().unwrap_or_else(|p| p.into_inner());
            for edit in edits {
                if let GuiEdit::Value(index, _) = edit {
                    if let Some(GuiEdit::Value(last, value)) = queue.last_mut() {
                        if *last == index {
                            *value = match edit {
                                GuiEdit::Value(_, v) => v,
                                _ => unreachable!(),
                            };
                            continue;
                        }
                    }
                }
                queue.push(edit);
            }
            if queue.is_empty() {
                return;
            }
        }
        if let Some(request_flush) = self.host_params.and_then(|p| p.request_flush) {
            unsafe { request_flush(self.host) };
        }
    }
}

unsafe fn host_extension<T>(host: *const clap_host, id: &CStr) -> Option<&'static T> {
    if host.is_null() {
        return None;
    }
    let get = unsafe { (*host).get_extension? };
    let ptr = unsafe { get(host, id.as_ptr()) } as *const T;
    unsafe { ptr.as_ref() }
}

/// State only the audio-thread callbacks touch (see the module doc).
struct Audio {
    engine: Option<Engine>,
    events: Vec<TimedEvent>,
    transport: Transport,
    sample_rate: f32,
    /// The right channel when the host gives a mono bus; sized at activate.
    mono_scratch: Vec<f32>,
}

#[repr(C)]
struct Instance {
    clap: clap_plugin,
    shared: Arc<Shared>,
    audio: UnsafeCell<Audio>,
    #[cfg(target_os = "macos")]
    editor: std::cell::RefCell<Option<crate::editor::Editor>>,
}

impl Instance {
    fn new(shell: Arc<Shell>, host: *const clap_host) -> Box<Self> {
        let params = Arc::new(ParamStore::new(shell.payload.spec.params.iter().map(|p| p.default)));
        Box::new(Self {
            clap: clap_plugin {
                desc: &shell.descriptor,
                plugin_data: std::ptr::null_mut(),
                init: Some(plugin_init),
                destroy: Some(plugin_destroy),
                activate: Some(plugin_activate),
                deactivate: Some(plugin_deactivate),
                start_processing: Some(plugin_start_processing),
                stop_processing: Some(plugin_stop_processing),
                reset: Some(plugin_reset),
                process: Some(plugin_process),
                get_extension: Some(plugin_get_extension),
                on_main_thread: Some(plugin_on_main_thread),
            },
            shared: Arc::new(Shared {
                shell,
                params,
                gui_edits: Mutex::new(Vec::with_capacity(256)),
                host,
                host_params: None,
            }),
            audio: UnsafeCell::new(Audio {
                engine: None,
                events: Vec::with_capacity(EVENT_CAPACITY),
                transport: Transport::free_running(48_000.0),
                sample_rate: 48_000.0,
                mono_scratch: Vec::new(),
            }),
            #[cfg(target_os = "macos")]
            editor: std::cell::RefCell::new(None),
        })
    }

    fn spec(&self) -> &crate::payload::PluginSpec {
        &self.shared.shell.payload.spec
    }

    /// # Safety
    /// Only from activate/deactivate/reset/process/flush, which CLAP never
    /// runs concurrently for one instance.
    #[allow(clippy::mut_from_ref)]
    unsafe fn audio(&self) -> &mut Audio {
        unsafe { &mut *self.audio.get() }
    }

    /// Drain editor edits into the host's output event queue.
    fn push_gui_edits(&self, out: *const clap_output_events) {
        if out.is_null() {
            return;
        }
        let Ok(mut queue) = self.shared.gui_edits.try_lock() else { return };
        let Some(try_push) = (unsafe { (*out).try_push }) else { return };
        let header = |size: usize, type_: u16| clap_event_header {
            size: size as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_,
            flags: 0,
        };
        for edit in queue.drain(..) {
            unsafe {
                match edit {
                    GuiEdit::Begin(index) | GuiEdit::End(index) => {
                        let type_ = if matches!(edit, GuiEdit::Begin(_)) {
                            CLAP_EVENT_PARAM_GESTURE_BEGIN
                        } else {
                            CLAP_EVENT_PARAM_GESTURE_END
                        };
                        let event = clap_event_param_gesture {
                            header: header(std::mem::size_of::<clap_event_param_gesture>(), type_),
                            param_id: self.shared.shell.param_ids[index],
                        };
                        try_push(out, &event.header);
                    }
                    GuiEdit::Value(index, value) => {
                        let event = clap_event_param_value {
                            header: header(std::mem::size_of::<clap_event_param_value>(), CLAP_EVENT_PARAM_VALUE),
                            param_id: self.shared.shell.param_ids[index],
                            cookie: std::ptr::null_mut(),
                            note_id: -1,
                            port_index: -1,
                            channel: -1,
                            key: -1,
                            value: value as f64,
                        };
                        try_push(out, &event.header);
                    }
                }
            }
        }
    }

    /// Translate the host's input events into `audio.events`, sorted by frame
    /// without allocating.
    fn collect_events(&self, audio: &mut Audio, input: *const clap_input_events) {
        audio.events.clear();
        if input.is_null() {
            return;
        }
        let (Some(size), Some(get)) = (unsafe { (*input).size }, unsafe { (*input).get }) else { return };
        let count = unsafe { size(input) };
        for i in 0..count {
            let header = unsafe { get(input, i) };
            let Some(h) = (unsafe { header.as_ref() }) else { continue };
            if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
                continue;
            }
            let Some(event) = self.translate(h, header) else { continue };
            if audio.events.len() < audio.events.capacity() || event.ends_a_note() {
                audio.events.push(TimedEvent { frame: h.time, event });
            }
        }
        // Hosts deliver events sorted; restore order in place if one did not.
        // Insertion sort is stable (a note-off before a same-frame note-on
        // stays first) and never allocates.
        if !audio.events.is_sorted_by_key(|e| e.frame) {
            for i in 1..audio.events.len() {
                let mut j = i;
                while j > 0 && audio.events[j - 1].frame > audio.events[j].frame {
                    audio.events.swap(j - 1, j);
                    j -= 1;
                }
            }
        }
    }

    fn translate(&self, h: &clap_event_header, header: *const clap_event_header) -> Option<Event> {
        Some(match h.type_ {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE => {
                let note = unsafe { &*(header as *const clap_event_note) };
                let (key, channel, note_id) = (note.key, note.channel, note.note_id);
                match h.type_ {
                    CLAP_EVENT_NOTE_ON if note.velocity > 0.0 => {
                        Event::Note(NoteEvent::On { key, channel, note_id, velocity: note.velocity as f32 })
                    }
                    CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => Event::Note(NoteEvent::Off { key, channel, note_id }),
                    _ => Event::Note(NoteEvent::Choke { key, channel }),
                }
            }
            CLAP_EVENT_MIDI => {
                let midi = unsafe { &*(header as *const clap_event_midi) };
                let status = midi.data[0] & 0xf0;
                let channel = (midi.data[0] & 0x0f) as i16;
                let key = midi.data[1] as i16;
                match status {
                    0x90 if midi.data[2] > 0 => Event::Note(NoteEvent::On {
                        key,
                        channel,
                        note_id: -1,
                        velocity: midi.data[2] as f32 / 127.0,
                    }),
                    0x80 | 0x90 => Event::Note(NoteEvent::Off { key, channel, note_id: -1 }),
                    0xb0 if midi.data[1] == 123 || midi.data[1] == 120 => Event::Note(NoteEvent::AllOff),
                    _ => return None,
                }
            }
            CLAP_EVENT_PARAM_VALUE => {
                let param = unsafe { &*(header as *const clap_event_param_value) };
                let index = self.shared.shell.param_index(param.param_id)?;
                Event::Param { index, value: param.value as f32 }
            }
            _ => return None,
        })
    }

    fn update_transport(audio: &mut Audio, transport: *const clap_event_transport) {
        let Some(t) = (unsafe { transport.as_ref() }) else { return };
        if t.flags & CLAP_TRANSPORT_HAS_TEMPO == 0 {
            return;
        }
        let beats_per_bar = if t.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0 && t.tsig_denom > 0 {
            t.tsig_num as f64 * 4.0 / t.tsig_denom as f64
        } else {
            4.0
        }
        .max(0.25);
        audio.transport.bar_inc = t.tempo / 60.0 / audio.sample_rate as f64 / beats_per_bar;
        if t.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 && t.flags & CLAP_TRANSPORT_IS_PLAYING != 0 {
            let pos = t.song_pos_beats as f64 / CLAP_BEATTIME_FACTOR as f64;
            let bar_start = t.bar_start as f64 / CLAP_BEATTIME_FACTOR as f64;
            audio.transport.bar_phase = ((pos - bar_start) / beats_per_bar).rem_euclid(1.0);
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let params: serde_json::Map<String, serde_json::Value> = self
            .spec()
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), serde_json::json!(self.shared.params.get(i))))
            .collect();
        let state = serde_json::json!({ "format": 1, "plugin": self.spec().id, "params": params });
        serde_json::to_vec(&state).unwrap_or_default()
    }

    fn load_state(&self, bytes: &[u8]) -> bool {
        let Ok(state) = serde_json::from_slice::<serde_json::Value>(bytes) else { return false };
        let Some(params) = state.get("params").and_then(|p| p.as_object()) else { return false };
        // Unknown names (from an older export) drop; missing ones default.
        for (i, spec) in self.spec().params.iter().enumerate() {
            let value = params.get(&spec.name).and_then(|v| v.as_f64()).map_or(spec.default, |v| v as f32);
            self.shared.params.set(i, spec.clamp(value));
        }
        true
    }
}

unsafe fn instance<'a>(plugin: *const clap_plugin) -> &'a Instance {
    unsafe { &*((*plugin).plugin_data as *const Instance) }
}

/// The state an instance shares with its editor. Main thread.
#[cfg(target_os = "macos")]
pub(crate) unsafe fn shared_of<'a>(plugin: *const clap_plugin) -> &'a Arc<Shared> {
    unsafe { &instance(plugin).shared }
}

/// The instance's editor slot. Main thread.
#[cfg(target_os = "macos")]
pub(crate) unsafe fn editor_slot<'a>(plugin: *const clap_plugin) -> &'a std::cell::RefCell<Option<crate::editor::Editor>> {
    unsafe { &instance(plugin).editor }
}

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    // Cache the host's params extension (the editor asks it to flush edits).
    let inst = unsafe { &mut *((*plugin).plugin_data as *mut Instance) };
    if let Some(shared) = Arc::get_mut(&mut inst.shared) {
        shared.host_params = unsafe { host_extension::<clap_host_params>(shared.host, CLAP_EXT_PARAMS) };
    }
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    unsafe { drop(Box::from_raw((*plugin).plugin_data as *mut Instance)) };
}

unsafe extern "C" fn plugin_activate(plugin: *const clap_plugin, sample_rate: f64, _min: u32, max: u32) -> bool {
    let inst = unsafe { instance(plugin) };
    let audio = unsafe { inst.audio() };
    audio.sample_rate = sample_rate as f32;
    audio.transport = Transport::free_running(audio.sample_rate);
    audio.mono_scratch = vec![0.0; max.max(1) as usize];
    let shell = &inst.shared.shell;
    match Engine::new(&shell.payload, &shell.lib, inst.shared.params.clone(), audio.sample_rate) {
        Ok(engine) => {
            audio.engine = Some(engine);
            true
        }
        Err(err) => {
            log(&format!("activate failed: {err}"));
            false
        }
    }
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    unsafe { instance(plugin).audio() }.engine = None;
}

unsafe extern "C" fn plugin_start_processing(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    if let Some(engine) = unsafe { instance(plugin).audio() }.engine.as_mut() {
        engine.reset();
    }
}

unsafe extern "C" fn plugin_process(plugin: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let inst = unsafe { instance(plugin) };
    let audio = unsafe { inst.audio() };
    let Some(process) = (unsafe { process.as_ref() }) else { return CLAP_PROCESS_ERROR };
    inst.collect_events(audio, process.in_events);
    Instance::update_transport(audio, process.transport);
    inst.push_gui_edits(process.out_events);
    let frames = process.frames_count as usize;
    if process.audio_outputs_count == 0 || process.audio_outputs.is_null() {
        return CLAP_PROCESS_CONTINUE;
    }
    let output = unsafe { &*process.audio_outputs };
    if output.data32.is_null() || output.channel_count == 0 {
        return CLAP_PROCESS_CONTINUE;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(*output.data32, frames) };
    let right_ptr = if output.channel_count > 1 { unsafe { *output.data32.add(1) } } else { std::ptr::null_mut() };
    let Some(engine) = audio.engine.as_mut() else {
        left.fill(0.0);
        return CLAP_PROCESS_CONTINUE;
    };
    if right_ptr.is_null() {
        // Mono bus: render stereo, fold to mono.
        let n = frames.min(audio.mono_scratch.len());
        let right = &mut audio.mono_scratch[..n];
        engine.process(&mut left[..n], right, &audio.events, &mut audio.transport);
        for (l, r) in left[..n].iter_mut().zip(right.iter()) {
            *l = 0.5 * (*l + *r);
        }
        left[n..].fill(0.0);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right_ptr, frames) };
        engine.process(left, right, &audio.events, &mut audio.transport);
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn plugin_on_main_thread(_: *const clap_plugin) {}

unsafe extern "C" fn plugin_get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else {
        #[cfg(target_os = "macos")]
        if id == clap_sys::ext::gui::CLAP_EXT_GUI {
            return crate::editor::gui_extension();
        }
        std::ptr::null()
    }
}

// ── audio + note ports ──────────────────────────────────────────────────

fn write_name<const N: usize>(dst: &mut [c_char; N], name: &str) {
    let bytes = name.as_bytes();
    let n = bytes.len().min(N - 1);
    for (d, &b) in dst.iter_mut().zip(&bytes[..n]) {
        *d = b as c_char;
    }
    dst[n] = 0;
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn audio_ports_count(_: *const clap_plugin, is_input: bool) -> u32 {
    if is_input { 0 } else { 1 }
}

unsafe extern "C" fn audio_ports_get(_: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    let Some(info) = (unsafe { info.as_mut() }) else { return false };
    if is_input || index != 0 {
        return false;
    }
    info.id = 0;
    write_name(&mut info.name, "Output");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = CLAP_INVALID_ID;
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

unsafe extern "C" fn note_ports_count(_: *const clap_plugin, is_input: bool) -> u32 {
    if is_input { 1 } else { 0 }
}

unsafe extern "C" fn note_ports_get(_: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    let Some(info) = (unsafe { info.as_mut() }) else { return false };
    if !is_input || index != 0 {
        return false;
    }
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    write_name(&mut info.name, "Notes");
    true
}

// ── params ──────────────────────────────────────────────────────────────

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

unsafe extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    unsafe { instance(plugin) }.spec().params.len() as u32
}

unsafe extern "C" fn params_get_info(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let inst = unsafe { instance(plugin) };
    let (Some(spec), Some(info)) = (inst.spec().params.get(index as usize), unsafe { info.as_mut() }) else {
        return false;
    };
    info.id = inst.shared.shell.param_ids[index as usize];
    info.flags = CLAP_PARAM_IS_AUTOMATABLE;
    if !spec.options.is_empty() {
        info.flags |= CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM;
    }
    info.cookie = std::ptr::null_mut();
    write_name(&mut info.name, &spec.label);
    write_name(&mut info.module, "");
    info.min_value = spec.min as f64;
    info.max_value = spec.max as f64;
    info.default_value = spec.default as f64;
    true
}

unsafe extern "C" fn params_get_value(plugin: *const clap_plugin, id: clap_id, value: *mut f64) -> bool {
    let inst = unsafe { instance(plugin) };
    let (Some(index), Some(value)) = (inst.shared.shell.param_index(id), unsafe { value.as_mut() }) else {
        return false;
    };
    *value = inst.shared.params.get(index) as f64;
    true
}

/// Host-facing text for a stored value: enum label, else the display value
/// (×100 for a percent ratio) and unit.
pub(crate) fn format_value(spec: &ParamSpec, value: f32) -> String {
    if !spec.options.is_empty() {
        return spec.option_label(value).map_or_else(|| format!("{}", value.round()), str::to_string);
    }
    let value = spec.to_display(value);
    let magnitude = value.abs();
    let text = if magnitude >= 100.0 {
        format!("{value:.0}")
    } else if magnitude >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    if spec.unit.is_empty() { text } else { format!("{text} {}", spec.unit) }
}

unsafe extern "C" fn params_value_to_text(
    plugin: *const clap_plugin,
    id: clap_id,
    value: f64,
    display: *mut c_char,
    size: u32,
) -> bool {
    let Some((_, spec)) = unsafe { instance(plugin) }.shared.shell.param(id) else { return false };
    if display.is_null() || size == 0 {
        return false;
    }
    let text = format_value(spec, value as f32);
    let bytes = text.as_bytes();
    let n = bytes.len().min(size as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, display, n);
        *display.add(n) = 0;
    }
    true
}

unsafe extern "C" fn params_text_to_value(
    plugin: *const clap_plugin,
    id: clap_id,
    display: *const c_char,
    value: *mut f64,
) -> bool {
    let Some((_, spec)) = unsafe { instance(plugin) }.shared.shell.param(id) else { return false };
    let Some(value) = (unsafe { value.as_mut() }) else { return false };
    if display.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(display) }.to_string_lossy();
    if let Some(i) = spec.options.iter().position(|o| o.eq_ignore_ascii_case(text.trim())) {
        *value = i as f64;
        return true;
    }
    let number: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
        .collect();
    match number.parse::<f32>() {
        Ok(v) => {
            *value = spec.clamp(spec.from_display(v)) as f64;
            true
        }
        Err(_) => false,
    }
}

unsafe extern "C" fn params_flush(plugin: *const clap_plugin, input: *const clap_input_events, output: *const clap_output_events) {
    let inst = unsafe { instance(plugin) };
    let audio = unsafe { inst.audio() };
    inst.collect_events(audio, input);
    for event in &audio.events {
        if let Event::Param { index, value } = event.event {
            inst.shared.params.set(index, value);
        }
    }
    audio.events.clear();
    inst.push_gui_edits(output);
}

// ── state ───────────────────────────────────────────────────────────────

static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let inst = unsafe { instance(plugin) };
    let Some(stream) = (unsafe { stream.as_ref() }) else { return false };
    let Some(write) = stream.write else { return false };
    let bytes = inst.save_state();
    let mut written = 0usize;
    while written < bytes.len() {
        let n = unsafe { write(stream, bytes[written..].as_ptr() as *const c_void, (bytes.len() - written) as u64) };
        if n <= 0 {
            return false;
        }
        written += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let inst = unsafe { instance(plugin) };
    let Some(stream) = (unsafe { stream.as_ref() }) else { return false };
    let Some(read) = stream.read else { return false };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = unsafe { read(stream, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u64) };
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
    inst.load_state(&bytes)
}
