/*!
DGenLisp side of the lisp host: the DSP-compile pipeline
(source → dgen → C → dylib → engine node) plus dylib caching and
instrument/effect storage. Everything is surfaced through the
`lisp_host` façade re-exports; nothing imports these modules directly.
*/

pub mod dgen_audit;
pub mod dgen_ffi;
// The rustfft-backed host-services table. Wired up off Apple only (Apple ships
// the vDSP table in audiograph/dgen_host_services.c), but compiled and tested
// everywhere so both backends stay pinned to the same reference DFT.
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
pub mod dgen_fft;
pub mod dgen_manifest;
pub mod dylib_cache;
pub mod effect_chain_graph;
pub mod effect_compile;
pub mod instrument_compile;
pub mod instrument_storage;

#[cfg(test)]
mod dgen_host_services_tests;
