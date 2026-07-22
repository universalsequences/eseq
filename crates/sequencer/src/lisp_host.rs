use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::vm::Value as EValue;
use eseqlisp::{CompileKind, Runtime};
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

pub mod dylib_cache;
mod graph_authoring;
mod graph_dsl;
mod graph_manifest;
mod graph_update;

pub use dylib_cache::{DGenCompileKind, DGenSourceOrigin, DylibLease};
pub use graph_authoring::register_graph_authoring_natives;
pub use graph_manifest::{graph_mode_present, parse_graph_manifest};
use graph_update::{CompiledGraphUpdate, SharedGraphNodeContext};

mod dgen_ffi;
mod dgen_manifest;
mod editor_flow;
mod effect_compile;
mod effect_chain_graph;
mod instrument_compile;
mod instrument_storage;
mod midi_fx;
mod native_arg_parsing;
mod neural_natives;
mod process_dsl_parse;
mod process_natives;
mod scratch_runtime;
mod sequencer_natives;
mod shared_state;
mod value_helpers;

pub use dgen_ffi::*;
#[cfg(test)]
use dgen_ffi::dgenlisp_wrapper_process;
pub use dgen_manifest::*;
pub use editor_flow::*;
pub use effect_compile::*;
pub use effect_chain_graph::*;
pub use instrument_compile::*;
pub use instrument_storage::*;
use midi_fx::*;
use native_arg_parsing::*;
pub use neural_natives::*;
use process_dsl_parse::*;
pub use process_natives::*;
pub use scratch_runtime::*;
pub use sequencer_natives::*;
pub use shared_state::*;
use value_helpers::*;

#[cfg(test)]
#[path = "lisp_host_tests.rs"]
mod tests;
