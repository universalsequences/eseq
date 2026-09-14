//! Positive and negative controls for the opt-in native sanitizer. Run these
//! in separate processes; a missing violation is a failed calibration.
use std::{ffi::CString, hint::black_box};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).expect("usage: audio_rtsan_probe clean|rust|native LIBRARY OP").clone();
    rtsan_standalone::ensure_initialized();
    let native = if mode == "native" {
        let path = CString::new(args.get(2).expect("native library path").as_str()).unwrap();
        let operation: u32 = args.get(3).expect("native operation").parse().unwrap();
        unsafe {
            let handle = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            assert!(!handle.is_null(), "native calibration library failed to load");
            let symbol = libc::dlsym(handle, c"eseq_allocation_control".as_ptr());
            assert!(!symbol.is_null(), "native calibration entry point missing");
            let function: unsafe extern "C" fn(u32) = std::mem::transmute(symbol);
            // Retain the library until process exit, outside the checked scope.
            Some((function, operation))
        }
    } else { None };
    std::thread::spawn(move || {
        let _realtime = rtsan_standalone::ScopedSanitizeRealtime::default();
        match mode.as_str() {
            "clean" => { black_box([0_u8; 1357]); }
            "rust" => { drop(black_box(vec![7_u8; black_box(1357)])); }
            "native" => unsafe {
                let (function, operation) = native.unwrap();
                function(operation);
            },
            _ => panic!("unknown calibration mode"),
        }
    }).join().unwrap();
}
