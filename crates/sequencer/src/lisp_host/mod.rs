/*!
The lisp host: façade over everything the sequencer crate layers on top of
the core lisp language.

Two language sides live in subfolders. `dgen/` is the DGenLisp DSP-compile
pipeline (lisp source → dgen → C → dylib → live audio-graph node, plus dylib
caching and instrument/effect storage). `eseq/` is the eseqlisp live-coding
surface (sequencer/process/neural natives, graph-mode authoring, the scratch
runtime, MIDI-FX event scripting). Shared plumbing both sides use — the
`Shared*` eval contexts and registries (`shared_state`), native argument
parsing, `EValue` construction helpers, and the embedded editor flow — stays
at this root.

This module is a pure façade: every submodule is surfaced through the
re-exports below, so in-crate consumers import from `crate::lisp_host::` and
never name the submodules directly (the lone exception is the
`lisp_host::dylib_cache` module path, kept alive by a module re-export).
*/

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::vm::Value as EValue;
use eseqlisp::{CompileKind, HostCommand, Runtime};
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

mod dgen;
mod eseq;

// Shared plumbing used by both language sides stays at the lisp_host root.
mod editor_flow;
mod native_arg_parsing;
mod shared_state;
mod value_helpers;

// -- dgenlisp: DSP-compile pipeline (source -> dgen -> dylib -> engine node) --
pub use dgen::dylib_cache; // module path compat: `lisp_host::dylib_cache::`
pub use dgen::dylib_cache::{DGenCompileKind, DGenSourceOrigin, DylibLease};
pub use dgen::dgen_ffi::*;
#[cfg(test)]
use dgen::dgen_ffi::dgenlisp_wrapper_process;
pub use dgen::dgen_manifest::*;
pub use dgen::effect_compile::*;
pub use dgen::effect_chain_graph::*;
pub use dgen::instrument_compile::*;
pub use dgen::instrument_storage::*;

// -- eseqlisp: live-coding / sequencing natives --
pub use eseq::graph_authoring::register_graph_authoring_natives;
pub use eseq::graph_manifest::{graph_mode_present, parse_graph_manifest};
use eseq::graph_update; // qualified `graph_update::` calls in shared_state/process_natives
use eseq::graph_update::{CompiledGraphUpdate, SharedGraphNodeContext};
use eseq::midi_fx::*;
pub use eseq::neural_natives::*;
use eseq::process_dsl_parse::*;
pub use eseq::process_natives::*;
pub use eseq::scratch_runtime::*;
pub use eseq::sequencer_natives::*;

pub use editor_flow::*;
use native_arg_parsing::*;
pub use shared_state::*;
use value_helpers::*;

#[cfg(test)]
mod tests;
