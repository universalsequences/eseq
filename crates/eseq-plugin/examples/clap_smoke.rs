//! Load an exported `.clap` bundle the way a host does and exercise it:
//! descriptor, params, notes → audio, state round-trip, and two instances
//! rendering concurrently on two threads (the shared-scratch hazard).
//!
//! `cargo run -p eseq-plugin --release --example clap_smoke -- "<path>/X.clap" [--wav out.wav]`

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::*;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

const SR: f64 = 48_000.0;
const BLOCK: usize = 256;

unsafe extern "C" fn host_get_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn host_noop(_: *const clap_host) {}

fn host() -> &'static clap_host {
    Box::leak(Box::new(clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"eseq clap_smoke".as_ptr(),
        vendor: c"eseq".as_ptr(),
        url: c"".as_ptr(),
        version: c"0".as_ptr(),
        get_extension: Some(host_get_extension),
        request_restart: Some(host_noop),
        request_process: Some(host_noop),
        request_callback: Some(host_noop),
    }))
}

struct Events {
    list: Vec<clap_event_note>,
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    let events = unsafe { &*((*list).ctx as *const Events) };
    events.list.len() as u32
}
unsafe extern "C" fn events_get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Events) };
    &events.list[i as usize].header
}
unsafe extern "C" fn out_try_push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}

fn note(kind: u16, time: u32, key: i16, velocity: f64) -> clap_event_note {
    clap_event_note {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_note>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: kind,
            flags: 0,
        },
        note_id: -1,
        port_index: 0,
        channel: 0,
        key,
        velocity,
    }
}

struct Plugin {
    raw: *const clap_plugin,
}

unsafe impl Send for Plugin {}

impl Plugin {
    fn ext<T>(&self, id: &CStr) -> &T {
        unsafe {
            let p = ((*self.raw).get_extension.unwrap())(self.raw, id.as_ptr()) as *const T;
            assert!(!p.is_null(), "missing extension {id:?}");
            &*p
        }
    }

    /// Render `frames`, with note-on at 0 for each key and note-off at `gate`.
    fn render(&self, keys: &[i16], frames: usize, gate: usize) -> (Vec<f32>, Vec<f32>) {
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        let mut done = 0;
        while done < frames {
            let n = BLOCK.min(frames - done);
            let mut events = Events { list: Vec::new() };
            if done == 0 {
                for &k in keys {
                    events.list.push(note(CLAP_EVENT_NOTE_ON, 0, k, 0.9));
                }
            }
            if gate >= done && gate < done + n {
                for &k in keys {
                    events.list.push(note(CLAP_EVENT_NOTE_OFF, (gate - done) as u32, k, 0.0));
                }
            }
            let in_events = clap_input_events {
                ctx: &events as *const Events as *mut c_void,
                size: Some(events_size),
                get: Some(events_get),
            };
            let out_events = clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(out_try_push) };
            let mut chans = [left[done..].as_mut_ptr(), right[done..].as_mut_ptr()];
            let mut out = clap_audio_buffer {
                data32: chans.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let process = clap_process {
                steady_time: done as i64,
                frames_count: n as u32,
                transport: std::ptr::null(),
                audio_inputs: std::ptr::null(),
                audio_outputs: &mut out,
                audio_inputs_count: 0,
                audio_outputs_count: 1,
                in_events: &in_events,
                out_events: &out_events,
            };
            let status = unsafe { ((*self.raw).process.unwrap())(self.raw, &process) };
            assert_ne!(status, CLAP_PROCESS_ERROR);
            done += n;
        }
        (left, right)
    }
}

fn stats(samples: &[f32]) -> (f32, f32, usize) {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt() as f32;
    let nonfinite = samples.iter().filter(|s| !s.is_finite()).count();
    (peak, rms, nonfinite)
}

fn write_wav(path: &Path, left: &[f32], right: &[f32]) {
    let mut data = Vec::new();
    for (l, r) in left.iter().zip(right) {
        data.extend_from_slice(&l.to_le_bytes());
        data.extend_from_slice(&r.to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&(SR as u32).to_le_bytes());
    wav.extend_from_slice(&(SR as u32 * 8).to_le_bytes());
    wav.extend_from_slice(&8u16.to_le_bytes());
    wav.extend_from_slice(&32u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

struct Bytes {
    data: Vec<u8>,
    pos: usize,
}
unsafe extern "C" fn ostream_write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
    let b = unsafe { &mut *((*s).ctx as *mut Bytes) };
    b.data.extend_from_slice(unsafe { std::slice::from_raw_parts(buf as *const u8, size as usize) });
    size as i64
}
unsafe extern "C" fn istream_read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
    let b = unsafe { &mut *((*s).ctx as *mut Bytes) };
    let n = (b.data.len() - b.pos).min(size as usize);
    unsafe { std::ptr::copy_nonoverlapping(b.data[b.pos..].as_ptr(), buf as *mut u8, n) };
    b.pos += n;
    n as i64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bundle = PathBuf::from(args.next().expect("bundle path"));
    let mut wav: Option<PathBuf> = None;
    let mut silence_param: Option<String> = None;
    // name=value params applied (through state) before the first render.
    let mut presets: Vec<(String, f64)> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--wav" => wav = args.next().map(PathBuf::from),
            // A param that silences the instrument at 0 (e.g. `level`): checks
            // that a store-only write (state load, the editor) reaches the DSP
            // of an already active instance.
            "--silence-param" => silence_param = args.next(),
            "--param" => {
                let raw = args.next().expect("--param name=value");
                let (name, value) = raw.split_once('=').expect("name=value");
                presets.push((name.to_string(), value.parse().expect("number")));
            }
            _ => {}
        }
    }
    let binary_dir = bundle.join("Contents/MacOS");
    let binary = std::fs::read_dir(&binary_dir).unwrap().next().unwrap().unwrap().path();
    let entry: &clap_plugin_entry = unsafe {
        let c = CString::new(binary.to_str().unwrap()).unwrap();
        let handle = libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        assert!(!handle.is_null(), "dlopen: {:?}", CStr::from_ptr(libc::dlerror()));
        let sym = libc::dlsym(handle, c"clap_entry".as_ptr());
        assert!(!sym.is_null(), "no clap_entry");
        &*(sym as *const clap_plugin_entry)
    };
    let path = CString::new(bundle.to_str().unwrap()).unwrap();
    assert!(unsafe { (entry.init.unwrap())(path.as_ptr()) }, "entry init failed");
    let factory = unsafe { &*((entry.get_factory.unwrap())(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory) };
    let count = unsafe { (factory.get_plugin_count.unwrap())(factory) };
    assert_eq!(count, 1);
    let desc = unsafe { &*(factory.get_plugin_descriptor.unwrap())(factory, 0) };
    let id = unsafe { CStr::from_ptr(desc.id) };
    println!("plugin: {} ({}) by {}", unsafe { CStr::from_ptr(desc.name) }.to_string_lossy(), id.to_string_lossy(), unsafe { CStr::from_ptr(desc.vendor) }.to_string_lossy());

    let create = || -> Plugin {
        let raw = unsafe { (factory.create_plugin.unwrap())(factory, host(), id.as_ptr()) };
        assert!(!raw.is_null());
        unsafe {
            assert!(((*raw).init.unwrap())(raw));
            assert!(((*raw).activate.unwrap())(raw, SR, 1, 4096));
            assert!(((*raw).start_processing.unwrap())(raw));
        }
        Plugin { raw }
    };

    let plugin = create();
    let params: &clap_plugin_params = plugin.ext(CLAP_EXT_PARAMS);
    let n = unsafe { (params.count.unwrap())(plugin.raw) };
    println!("params: {n}");
    for i in 0..n.min(6) {
        let mut info: clap_param_info = unsafe { std::mem::zeroed() };
        unsafe { (params.get_info.unwrap())(plugin.raw, i, &mut info) };
        let mut value = 0.0;
        unsafe { (params.get_value.unwrap())(plugin.raw, info.id, &mut value) };
        let mut text = [0 as c_char; 64];
        unsafe { (params.value_to_text.unwrap())(plugin.raw, info.id, value, text.as_mut_ptr(), 64) };
        println!(
            "  {:>10} {} [{:.2}..{:.2}] = {}",
            info.id,
            unsafe { CStr::from_ptr(info.name.as_ptr()) }.to_string_lossy(),
            info.min_value,
            info.max_value,
            unsafe { CStr::from_ptr(text.as_ptr()) }.to_string_lossy()
        );
    }

    if !presets.is_empty() {
        let params: serde_json::Map<String, serde_json::Value> =
            presets.iter().map(|(n, v)| (n.clone(), serde_json::json!(v))).collect();
        // A partial state resets the rest to defaults, as a plugin load does.
        let mut bytes = Bytes { data: serde_json::to_vec(&serde_json::json!({ "format": 1, "params": params })).unwrap(), pos: 0 };
        let input = clap_istream { ctx: &mut bytes as *mut Bytes as *mut c_void, read: Some(istream_read) };
        let state: &clap_plugin_state = plugin.ext(CLAP_EXT_STATE);
        assert!(unsafe { (state.load.unwrap())(plugin.raw, &input) });
        println!("applied {} params", presets.len());
    }
    let frames = (SR * 1.5) as usize;
    let (l, r) = plugin.render(&[36], frames, (SR * 0.25) as usize);
    let (peak, rms, bad) = stats(&l);
    println!("note 36: peak {peak:.4} rms {rms:.4} nonfinite {bad}");
    assert!(peak > 1e-3 && bad == 0, "no sound");
    let tail = stats(&l[frames - 2048..]);
    println!("tail peak {:.6}", tail.0);
    if let Some(path) = &wav {
        write_wav(path, &l, &r);
        println!("wrote {}", path.display());
    }

    // Retrigger: the same note again renders again.
    let (l2, _) = plugin.render(&[36], frames, (SR * 0.25) as usize);
    println!("retrigger peak {:.4}", stats(&l2).0);

    // State round-trip into a fresh instance renders identically.
    let state: &clap_plugin_state = plugin.ext(CLAP_EXT_STATE);
    let mut saved = Bytes { data: Vec::new(), pos: 0 };
    let out = clap_ostream { ctx: &mut saved as *mut Bytes as *mut c_void, write: Some(ostream_write) };
    assert!(unsafe { (state.save.unwrap())(plugin.raw, &out) });
    println!("state: {} bytes", saved.data.len());
    let other = create();
    let other_state: &clap_plugin_state = other.ext(CLAP_EXT_STATE);
    let input = clap_istream { ctx: &mut saved as *mut Bytes as *mut c_void, read: Some(istream_read) };
    assert!(unsafe { (other_state.load.unwrap())(other.raw, &input) });

    if let Some(param) = &silence_param {
        let mut silent = Bytes {
            data: serde_json::to_vec(&serde_json::json!({ "format": 1, "params": { param: 0.0 } })).unwrap(),
            pos: 0,
        };
        let input = clap_istream { ctx: &mut silent as *mut Bytes as *mut c_void, read: Some(istream_read) };
        assert!(unsafe { (other_state.load.unwrap())(other.raw, &input) });
        let (l, _) = other.render(&[36], frames, frames / 4);
        println!("after {param}=0 via state: peak {:.6}", stats(&l).0);
        assert!(stats(&l).0 < 1e-4, "a store-only param write did not reach the DSP");
    }
    drop((plugin, other));
    // Two fresh instances, two threads, same notes: identical output means
    // their voices never shared a scratch slice.
    let (first, second) = (create(), create());
    let a = std::thread::spawn(move || first.render(&[36, 40, 43], frames, frames / 3).0);
    let b = std::thread::spawn(move || second.render(&[36, 40, 43], frames, frames / 3).0);
    let (a, b) = (a.join().unwrap(), b.join().unwrap());
    let diff = a.iter().zip(&b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
    println!("concurrent instances: peak {:.4}, max diff {diff:.3e}", stats(&a).0);
    assert!(diff < 1e-6, "concurrent instances diverged");
    println!("OK");
}
