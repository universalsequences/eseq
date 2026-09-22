//! Graph node process patches (`docs/graph-node-processes-spec.md`).
//!
//! The scheduler drives each graph runtime through `SchedulerGraphDriver`: the
//! node-rule predicate plus the emit-time hook that runs a node's process patch
//! on the fire payload before it is emitted *and* scattered. Step-param writes
//! (`transpose` / `velocity` / `duration`) edit the payload; `veto!` mutes the
//! emission only (the fire still scatters, zeroes energy and advances). Other
//! commands (ratchet, roll, device writes, graph control) are ignored on nodes
//! in this first cut.

use super::*;
use super::process::{
    ProcessInletWriteContext, ProcessTargetOverlay, apply_process_target_writes,
    apply_step_process_commands, invoke_process_cascade, process_step_event_value,
};
use crate::graph::{GraphDriver, GraphPayload, NodeEmitContext, NodeEval, NodeFire};
use crate::runtime::generator::default_resolved;

pub(super) struct SchedulerGraphDriver<'a> {
    pub scratch_runtime: &'a mut Option<lisp_host::ScratchControlRuntime>,
    pub process_runtime: &'a mut crate::process::ProcessRuntime,
    pub snapshot: &'a SequencerSnapshot,
    pub manifest: &'a crate::graph::GraphManifest,
    pub debug_graph: bool,
    pub debug_accum: bool,
    pub eval_count: usize,
    /// The delay change the last patch run asked for; handed to the runtime
    /// through `take_delay_offset_steps` right after `emit`.
    pub last_delay_offset_steps: f32,
    /// `graph-reset!` requests from the last patch run (`None` = whole graph).
    pub pending_resets: Vec<Option<u8>>,
}

impl GraphDriver for SchedulerGraphDriver<'_> {
    fn update(&mut self, eval: &NodeEval) -> NodeFire {
        self.eval_count += 1;
        let Some(scratch) = self.scratch_runtime.as_mut() else {
            return NodeFire::default();
        };
        match scratch.invoke_graph_update(self.manifest, eval) {
            Ok(decision) => decision,
            Err(error) => {
                if self.debug_graph {
                    eprintln!(
                        "[graph-update-error] graph={} node={} beat={:.6} error={}",
                        self.manifest.name, eval.node_index, eval.beat, error
                    );
                }
                NodeFire::default()
            }
        }
    }

    fn emit(&mut self, ctx: &NodeEmitContext, payload: &mut GraphPayload) -> bool {
        let outcome = run_node_process_patch(
            self.scratch_runtime,
            self.process_runtime,
            self.snapshot,
            self.manifest,
            ctx,
            payload,
            self.debug_accum,
        );
        self.last_delay_offset_steps = outcome.delay_offset_steps;
        self.pending_resets = outcome.reset_requests;
        outcome.audible
    }

    fn take_delay_offset_steps(&mut self) -> f32 {
        std::mem::take(&mut self.last_delay_offset_steps)
    }

    fn take_reset_requests(&mut self) -> Vec<Option<u8>> {
        std::mem::take(&mut self.pending_resets)
    }
}

/// What one patch run asked of the fire beyond the payload edits.
pub(super) struct NodePatchOutcome {
    pub audible: bool,
    pub delay_offset_steps: f32,
    pub reset_requests: Vec<Option<u8>>,
}

/// Run one node's process patch on its fire payload. Returns whether the
/// emission is audible, the delay change (steps) the patch wrote through the
/// `delay` payload field, and any `graph-reset!` requests.
///
/// The patch sees a synthesized step context (spec §2.2): the route track, the
/// destination-track step the fire lands on (for RNG seeding and `(step)`),
/// and a `ResolvedStep` carrying the payload's transpose / velocity / duration.
/// Lane inlets with no painted lane fall back to the slot's scalar value, which
/// is how every inlet is a knob on a node (spec §2.4). Wires between slots use
/// the track patch bay's inlet-write plumbing verbatim: a connectable port
/// bound to a later slot's inlet writes that inlet for this fire.
pub(super) fn run_node_process_patch(
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    process_runtime: &mut crate::process::ProcessRuntime,
    snapshot: &SequencerSnapshot,
    manifest: &crate::graph::GraphManifest,
    ctx: &NodeEmitContext,
    payload: &mut GraphPayload,
    debug_accum: bool,
) -> NodePatchOutcome {
    let track = ctx.route.unwrap_or(0);
    let num_steps = snapshot
        .tracks
        .get(track)
        .map(|track| track.params.num_steps)
        .unwrap_or(16)
        .max(1);
    let step_beats = ctx.step_beats.max(1e-6);
    let step_index = (ctx.beat / step_beats as f64).floor().max(0.0) as u64;
    let step = (step_index % num_steps as u64) as usize;
    let cycle = step_index / num_steps as u64;
    let samples_per_step = step_beats * 48_000.0 * 60.0 / (snapshot.transport.bpm.max(1) as f32);

    let mut resolved = default_resolved();
    resolved.transpose = payload.note;
    resolved.velocity = payload.velocity;
    resolved.duration = payload.duration_beats;
    let mut audible = true;
    let mut overlay = ProcessTargetOverlay::default();
    let chain = &ctx.process_chain;
    let mut inlet_writes = process_runtime.take_step_process_inlet_writes(track, chain);
    // `(read (neuron k ...))` inside this patch sees the graph's recent notes.
    process_runtime.set_neuron_reads(ctx.neuron_recent_notes.clone());
    let mut deferred_writes = Vec::new();
    let mut reset_requests: Vec<Option<u8>> = Vec::new();

    for (slot_index, slot) in chain.slots.iter().enumerate() {
        if !slot.enabled {
            continue;
        }
        let slot_inlet_writes = inlet_writes.remove(&slot_index).unwrap_or_default();
        // Fixed target writes (accumulator-style slots) before the run body.
        let writes = process_runtime.step_process_writes_with_inlet_writes(
            slot,
            step,
            cycle,
            num_steps,
            Some(&slot_inlet_writes),
        );
        {
            let mut inlet_context = ProcessInletWriteContext {
                chain,
                current_slot_index: Some(slot_index),
                current_fire_writes: &mut inlet_writes,
                deferred_writes: &mut deferred_writes,
            };
            apply_process_target_writes(
                snapshot,
                &[],
                track,
                step,
                &mut resolved,
                &mut overlay,
                Some(slot),
                &writes,
                Some(&mut inlet_context),
            );
        }
        let mut event = process_step_event_value(
            track,
            step,
            cycle,
            ctx.beat,
            ctx.sample_time,
            resolved,
            step_beats,
        );
        if let eseqlisp::vm::Value::Map(map) = &mut event {
            map.insert(
                "node".to_string(),
                Rc::new(RefCell::new(eseqlisp::vm::Value::Number(ctx.node_index as f64))),
            );
            map.insert(
                "graph".to_string(),
                Rc::new(RefCell::new(eseqlisp::vm::Value::String(manifest.name.clone()))),
            );
        }
        let Some(invocation) = process_runtime.step_process_invocation_with_inlet_writes(
            slot,
            crate::process::ProcessStepRunContext {
                track,
                step,
                cycle,
                beat: ctx.beat,
                sample_time: ctx.sample_time,
                step_beats,
                resolved,
                note: payload.note,
                event,
                fire_seed: Some(ctx.sample_time),
                after_reset: ctx.after_reset,
            },
            Some(&slot_inlet_writes),
        ) else {
            continue;
        };
        let completed = invoke_process_cascade(
            scratch_runtime,
            process_runtime,
            invocation,
            debug_accum,
            |scratch, process_runtime, runtime_id, commands| {
                let mut inlet_context = ProcessInletWriteContext {
                    chain,
                    current_slot_index: Some(slot_index),
                    current_fire_writes: &mut inlet_writes,
                    deferred_writes: &mut deferred_writes,
                };
                // `graph-reset!` is the one graph command a node honours:
                // it targets the node's own graph, so the runner collects it
                // here and the runtime applies it after this boundary commits.
                for command in commands {
                    if let crate::process::ProcessRunCommand::Graph(
                        crate::graph::GraphControlCommand::Reset { group, .. },
                    ) = command
                    {
                        reset_requests.push(*group);
                    }
                }
                // Step-param / device / process-inlet writes and veto apply
                // exactly as on a track step. Ratchet, roll and the other
                // graph-control commands are ignored on nodes.
                apply_step_process_commands(
                    scratch,
                    process_runtime,
                    runtime_id,
                    snapshot,
                    &[],
                    track,
                    step,
                    ctx.beat,
                    samples_per_step,
                    Some(slot),
                    &mut resolved,
                    &mut overlay,
                    &mut audible,
                    commands,
                    Some(&mut inlet_context),
                    debug_accum,
                );
            },
        );
        if !completed {
            break;
        }
    }
    process_runtime.set_neuron_reads(Vec::new());
    for deferred in deferred_writes.drain(..) {
        process_runtime.defer_step_process_inlet_write(
            deferred.track,
            deferred.instance_id,
            deferred.inlet,
            deferred.write,
        );
    }

    payload.note = resolved.transpose;
    payload.velocity = resolved.velocity;
    payload.duration_beats = resolved.duration;
    NodePatchOutcome {
        audible,
        delay_offset_steps: overlay.node_delay_offset_steps,
        reset_requests,
    }
}
