//! Eseqlisp side of the lisp host: live-coding / sequencing natives —
//! sequencer + process + neural natives, graph-mode authoring, the scratch
//! runtime, and MIDI-FX event scripting. Everything is surfaced through the
//! `lisp_host` façade re-exports; nothing imports these modules directly.

pub mod graph_authoring;
pub mod graph_dsl;
pub mod graph_manifest;
pub mod graph_update;
pub mod midi_fx;
pub mod neural_natives;
pub mod process_dsl_parse;
pub mod process_natives;
pub mod scratch_runtime;
pub mod sequencer_natives;
