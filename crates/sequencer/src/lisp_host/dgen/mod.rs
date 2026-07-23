//! DGenLisp side of the lisp host: the DSP-compile pipeline
//! (source → dgen → C → dylib → engine node) plus dylib caching and
//! instrument/effect storage. Everything is surfaced through the
//! `lisp_host` façade re-exports; nothing imports these modules directly.

pub mod dgen_ffi;
pub mod dgen_manifest;
pub mod dylib_cache;
pub mod effect_chain_graph;
pub mod effect_compile;
pub mod instrument_compile;
pub mod instrument_storage;
