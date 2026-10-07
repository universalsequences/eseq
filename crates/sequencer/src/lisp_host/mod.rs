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
pub use dgen::dylib_cache::{DGenCompileKind, DGenSourceOrigin, DylibLease, DGEN_INSTRUMENT_VOICES};
pub use dgen::dgen_ffi::*;
#[cfg(test)]
use dgen::dgen_ffi::dgenlisp_wrapper_process;
pub use dgen::dgen_manifest::*;
pub use dgen::effect_compile::*;
pub use dgen::effect_chain_graph::*;
pub use dgen::instrument_compile::*;
pub use dgen::instrument_storage::*;
pub use dgen::probe_capture::{
    bind_effect_probe_node, clear_dgen_instrument_probes, clear_effect_probes, dgen_display_voice,
    note_dgen_display_voice, probe_infos, probe_scope_window, probe_set, probe_snapshot,
    probes_watched, publish_dgen_instrument_probes, reclaim_retired_probe_sets,
    register_effect_probes, release_effect_probe_token, unwatch_probes, watch_probes,
    EffectProbeToken, ProbeInstance, ProbeReading, ProbeSetHandle, ProbeWatchGuard,
    MAX_EFFECT_PROBE_INSTANCES, PROBE_SCOPE_POINTS_PER_BLOCK, PROBE_SCOPE_RING_PAIRS,
};
pub use eseqlisp::live_audio::ProbeView;

/// Resolve the optional top-level `(effect-latency …)` declaration of an
/// effect source at `sample_rate`, without compiling it. Builtin modules use
/// this to pin their declared latency in tests.
pub(crate) fn declared_effect_latency_samples(source: &str, sample_rate: u32) -> Result<Option<u32>, String> {
    dgen::effect_latency::prepare(source, sample_rate, true).map(|(_, samples)| samples)
}

// -- eseqlisp: live-coding / sequencing natives --
pub use eseq::graph_authoring::{
    GRAPH_NODE_HIDDEN_PROCESS_CLASSES, GRAPH_NODE_LANE_PATCH_NAMESPACE_BASE, GRAPH_NODE_PAYLOAD_FIELDS,
    GRAPH_NODE_PROCESS_HISTORY_COMMAND,
    GRAPH_READ_REACTIVE_NAMESPACE, GraphNodeProcessReminter, graph_node_lane_patch_namespace,
    queue_graph_read_invalidations, register_graph_authoring_natives,
    restore_graph_node_process_chain, ensure_graph_overrides, graph_config_field_value,
    graph_edge_param_value, graph_node_intrinsic_value, graph_node_param_value,
    graph_seed_follows_route, published_graph_manifest, resolved_graph_overrides_for_manifest,
    restore_graph_override, edit_graph_node_process_chain_now, ensure_graph_node_expr_classes,
    graph_node_process_inlet_merge_key, graph_node_process_label,
    process_slot_as_expr,
};
pub use eseq::graph_manifest::{
    current_graph_owner_rack, graph_instance_id, graph_mode_present, parse_graph_manifest,
    parse_graph_manifest_owned, with_graph_owner_rack,
};
pub use eseq::kinds::{
    DEF_KIND_DOCS, DEF_KIND_KEYWORDS, DEF_KIND_SIGNATURE, DeclaredKind, KindDefinition,
    SCRATCH_KIND_PACKAGE, declared_kinds_for_module, declared_module_for_kind,
    check_manifest_kinds, clear_kind_registry, drop_all_instance_records, instance_owner_value,
    instance_published_sequencer, instance_sequencer_name, sync_instance_records, kind_id,
    instance_document_slot, instance_document_slot_owner, instance_generator_tick_source,
    INSTANCE_DOCUMENT_NATIVE, INSTANCE_DOCUMENT_SLOT_PREFIX,
    kind_name_of, kind_package_of, kind_registry_version,
    kinds_defined_in_module, package_name_for_module, parse_def_kind, register_def_kind_native,
    kind_is_registered, register_kind, registered_kind, registered_kinds, unregister_module_kinds,
    InstanceView, InstanceViewChange, desired_instance_views, instance_key_scope,
    instance_view_buffer_name, run_instance_on_create, sync_instance_view_buffers,
};
pub use eseq::expr_process::{
    CompiledExpr, EXPR_CONTEXT_VARS, EXPR_INLET_MAX, EXPR_INLET_MIN, EXPR_MAX_NESTING, ExprAnalysis,
    ExprCompileError, ExprRebind, analyze_expr_source, compile_and_register_expr_source,
    compile_expr_source, expr_class_name_for_source, expr_sources_in_scenes, is_expr_slot,
    rebind_expr_slot, rebind_slot_class, rederive_expr_class_names_in_chain, sync_expr_process_classes,
};
use eseq::graph_update; // qualified `graph_update::` calls in shared_state/process_natives
use eseq::graph_update::{CompiledGraphUpdate, SharedGraphNodeContext};
use eseq::midi_fx::*;
pub use eseq::neural_natives::*;
use eseq::process_dsl_parse::*;
pub use eseq::process_natives::*;
pub use eseq::process_library::{
    is_my_processes_source, load_my_processes_source, my_processes_module_files,
    my_processes_package_dir, set_my_processes_package_dir_override, MyProcessesDirOverrideGuard,
    MY_PROCESSES_MODULE_PREFIX,
    MY_PROCESSES_PACKAGE,
};
pub use eseq::expr_promote::{
    promoted_class_name, promoted_process_module_source, validate_promote_name,
    write_promoted_process, PromoteTarget,
};
pub use eseq::scratch_runtime::*;
pub use eseq::sequencer_natives::*;

pub use editor_flow::*;
use native_arg_parsing::*;
pub use shared_state::*;
use value_helpers::*;

#[cfg(test)]
mod tests;
