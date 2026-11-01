//! The DGen ABI v1 surface between a host and a compiled DGenLisp dylib,
//! shared by eseq (`sequencer::lisp_host`) and exported plugins
//! (`eseq-plugin`): the `#[repr(C)]` mirrors of `audiograph/dgen_abi_v1.h`
//! (vendored from the staged toolchain's `include/dgen_runtime.h`), the
//! process-fn type, the host-services tables (`audiograph/dgen_host_services.c`,
//! compiled by this crate's build script) and the dylib loader.

use std::ffi::{c_int, c_void, CStr, CString};
use std::path::Path;

pub const DGEN_ABI_VERSION_V1: u32 = 1;

/// Manifest `processAbi` value the vendored DGenLisp emits for ABI v1.
pub const DGEN_PROCESS_ABI_V1: &str = "dgen-host-abi-v1";

/// Slots after a node's memory span the host keeps zeroed: generated code
/// may overrun its declared span by up to this much.
pub const DGEN_STATE_REDZONE_SLOTS: usize = 256;

pub type DGenFFTSetupV1 = *mut c_void;

#[repr(C)]
pub struct DGenProcessContextV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub sample_rate: f32,
    pub reserved: u32,
}

impl DGenProcessContextV1 {
    pub fn v1(sample_rate: f32) -> Self {
        Self {
            abi_version: DGEN_ABI_VERSION_V1,
            struct_size: std::mem::size_of::<Self>() as u32,
            sample_rate,
            reserved: 0,
        }
    }
}

#[repr(C)]
pub struct DGenHostServicesV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub fft_setup_create_fn: Option<unsafe extern "C" fn(log2_size: u32) -> DGenFFTSetupV1>,
    pub fft_forward_fn: Option<
        unsafe extern "C" fn(
            setup: DGenFFTSetupV1,
            real: *mut f32,
            imaginary: *mut f32,
            log2_size: u32,
        ),
    >,
    pub fft_inverse_fn: Option<
        unsafe extern "C" fn(
            setup: DGenFFTSetupV1,
            real: *mut f32,
            imaginary: *mut f32,
            log2_size: u32,
        ),
    >,
    pub complex_multiply_accumulate_fn: Option<
        unsafe extern "C" fn(
            lhs_real: *const f32,
            lhs_imaginary: *const f32,
            rhs_real: *const f32,
            rhs_imaginary: *const f32,
            accumulator_real: *mut f32,
            accumulator_imaginary: *mut f32,
            element_count: u32,
        ),
    >,
}

/// `dgen_process_v1` — the ABI v1 export of a compiled DGen dylib. `state` is
/// the single `float *memory` span (generated code casts it directly).
pub type DGenProcessFn = unsafe extern "C" fn(
    inputs: *const *const f32,
    outputs: *const *mut f32,
    frame_count: u32,
    state: *mut c_void,
    context: *const DGenProcessContextV1,
    host: *const DGenHostServicesV1,
);

extern "C" {
    fn eseq_dgen_portable_host_services_v1() -> *const DGenHostServicesV1;

    #[cfg(target_vendor = "apple")]
    fn eseq_dgen_host_services_v1() -> *const DGenHostServicesV1;
}

/// The portable C host-services table; process-lifetime static on every
/// target.
pub fn portable_host_services_v1() -> *const DGenHostServicesV1 {
    unsafe { eseq_dgen_portable_host_services_v1() }
}

/// The Accelerate/vDSP-backed table (`audiograph/dgen_host_services.c`);
/// process-lifetime static.
#[cfg(target_vendor = "apple")]
pub fn accelerate_host_services_v1() -> *const DGenHostServicesV1 {
    unsafe { eseq_dgen_host_services_v1() }
}

/// A dlopen'ed DGen dylib. The handle is never closed: generated images own
/// file-scope statics (FFT setups, voice scratch) that outlive any one user.
pub struct DGenLibrary {
    pub handle: *mut c_void,
    pub process: DGenProcessFn,
}

/// Open the dylib at `path` with `dlopen` `mode` (`RTLD_NOW`, plus
/// `RTLD_LOCAL` to keep its symbols private) and resolve the ABI v1 exports.
/// `dgen_set_param_value_v1` is a no-op stub today (hosts write state memory
/// directly) but is resolved too, so a dylib missing the export is caught at
/// load.
pub fn open_library(path: &Path, mode: c_int) -> Result<DGenLibrary, String> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| format!("bad library path {}", path.display()))?;
    unsafe {
        let last_error = || {
            let err = libc::dlerror();
            if err.is_null() {
                "unknown error".to_string()
            } else {
                CStr::from_ptr(err).to_string_lossy().into_owned()
            }
        };
        let handle = libc::dlopen(c_path.as_ptr(), mode);
        if handle.is_null() {
            return Err(format!("dlopen {}: {}", path.display(), last_error()));
        }
        let process = libc::dlsym(handle, c"dgen_process_v1".as_ptr());
        if process.is_null() {
            return Err(format!("dlsym 'dgen_process_v1' failed: {}", last_error()));
        }
        if libc::dlsym(handle, c"dgen_set_param_value_v1".as_ptr()).is_null() {
            return Err(format!("dlsym 'dgen_set_param_value_v1' failed: {}", last_error()));
        }
        Ok(DGenLibrary { handle, process: std::mem::transmute::<*mut c_void, DGenProcessFn>(process) })
    }
}
