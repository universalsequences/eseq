/*!
Graph runtime reconciliation plus neural, generator, and graph emission merging.
*/

#[allow(unused_imports)]
use super::*;

/// Seed every graph-mode runtime from a step-sequencer trigger (spec §4): each node
/// whose resolved `seed-from` includes the event's track receives a fire carrying the
/// step's note/velocity, after that node's delay. Mirrors the `neural` seed sites.
pub(super) fn seed_graph_runtimes(
    graphs: &mut [crate::graph::GraphRuntime],
    event: &StepEvent,
    seed_beats: f64,
    samples_per_quarter: f64,
) {
    if graphs.is_empty() {
        return;
    }
    let duration_beats = if samples_per_quarter.is_finite() && samples_per_quarter > 0.0 {
        let value =
            event.resolved.duration as f64 * event.samples_per_step as f64 / samples_per_quarter;
        if value.is_finite() {
            value.max(0.0) as f32
        } else {
            0.0
        }
    } else {
        0.0
    };
    let payload = crate::graph::GraphPayload {
        note: event.resolved.transpose,
        velocity: event.resolved.velocity,
        duration_beats,
    };
    let debug_graph = std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some();
    for (graph_idx, graph) in graphs.iter_mut().enumerate() {
        let seeded = graph.seed(event.track, seed_beats, payload);
        if debug_graph {
            eprintln!(
                "[graph-seed] graph={} track={} beat={:.6} seeded={} node0_mask={:#x} node0_pending={}",
                graph_idx,
                event.track,
                seed_beats,
                seeded,
                graph.seed_track_mask_for_node(0).unwrap_or(0),
                graph.pending_count_for_node(0).unwrap_or(0)
            );
        }
    }
}

pub(super) fn publish_graph_visualizations(
    state: &SequencerState,
    graphs: &[crate::graph::GraphRuntime],
    current_beat: f64,
) {
    state.set_graph_visualizations(
        graphs
            .iter()
            .map(|graph| graph.visualization_snapshot_at(current_beat))
            .collect(),
    );
}

pub(super) fn same_coincident_note(
    existing_sample_time: u64,
    existing_track: Option<usize>,
    existing_transpose: f32,
    incoming_sample_time: u64,
    incoming_track: Option<usize>,
    incoming_transpose: f32,
) -> bool {
    existing_sample_time == incoming_sample_time
        && existing_track == incoming_track
        && existing_transpose == incoming_transpose
}

pub(super) fn record_track_output_event(
    events: &mut Vec<TrackOutputEvent>,
    track: usize,
    sample_time: u64,
    beat: f64,
    resolved: ResolvedStep,
) {
    events.push(TrackOutputEvent {
        track,
        sample_time,
        beat,
        transpose: resolved.transpose,
        velocity: resolved.velocity,
    });
}

pub(super) fn neural_outputs_are_same_accent(existing: &NeuralOutput, incoming: &NeuralOutput) -> bool {
    same_coincident_note(
        existing.sample_time,
        Some(existing.event.track),
        existing.event.resolved.transpose,
        incoming.sample_time,
        Some(incoming.event.track),
        incoming.event.resolved.transpose,
    )
}

pub(super) fn merge_neural_output_accents(neural_events: Vec<NeuralOutput>) -> Vec<NeuralOutput> {
    let mut merged: Vec<NeuralOutput> = Vec::with_capacity(neural_events.len());
    for output in neural_events {
        if output.emit_trigger {
            if let Some(existing) = merged.iter_mut().find(|existing| {
                existing.emit_trigger && neural_outputs_are_same_accent(existing, &output)
            }) {
                existing.event.resolved.velocity =
                    (existing.event.resolved.velocity + output.event.resolved.velocity).min(1.0);
                continue;
            }
        }
        merged.push(output);
    }
    merged
}

pub(super) fn generator_emissions_are_same_accent(
    existing: &crate::generator::GeneratorEmission,
    incoming: &crate::generator::GeneratorEmission,
) -> bool {
    same_coincident_note(
        existing.sample_time,
        existing.event.track,
        existing.event.resolved.transpose,
        incoming.sample_time,
        incoming.event.track,
        incoming.event.resolved.transpose,
    )
}

pub(super) fn merge_generator_emission_accents(
    generator_emissions: Vec<crate::generator::GeneratorEmission>,
) -> Vec<crate::generator::GeneratorEmission> {
    let mut merged: Vec<crate::generator::GeneratorEmission> =
        Vec::with_capacity(generator_emissions.len());
    for emission in generator_emissions {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| generator_emissions_are_same_accent(existing, &emission))
        {
            existing.event.resolved.velocity =
                (existing.event.resolved.velocity + emission.event.resolved.velocity).min(1.0);
            continue;
        }
        merged.push(emission);
    }
    merged
}

pub(super) fn graph_emissions_are_same_accent(
    existing: &crate::graph::GraphEmission,
    incoming: &crate::graph::GraphEmission,
) -> bool {
    same_coincident_note(
        existing.sample_time,
        existing.event.track,
        existing.event.resolved.transpose,
        incoming.sample_time,
        incoming.event.track,
        incoming.event.resolved.transpose,
    )
}

pub(super) fn merge_graph_emission_accents(
    graph_emissions: Vec<crate::graph::GraphEmission>,
) -> Vec<crate::graph::GraphEmission> {
    let mut merged: Vec<crate::graph::GraphEmission> = Vec::with_capacity(graph_emissions.len());
    for emission in graph_emissions {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| graph_emissions_are_same_accent(existing, &emission))
        {
            existing.event.resolved.velocity =
                (existing.event.resolved.velocity + emission.event.resolved.velocity).min(1.0);
            continue;
        }
        merged.push(emission);
    }
    merged
}

pub(super) fn graph_overrides_for_manifest<'a>(
    manifest: &crate::graph::GraphManifest,
    overrides: &'a [crate::graph::ProjectGraphOverrides],
) -> Option<&'a crate::graph::ProjectGraphOverrides> {
    overrides.iter().find(|overrides| {
        overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
    })
}

pub(super) fn reconcile_graph_runtimes(
    manifests: Vec<crate::graph::GraphManifest>,
    overrides: &[crate::graph::ProjectGraphOverrides],
    runtimes: &mut Vec<crate::graph::GraphRuntime>,
    stored_manifests: &mut Vec<crate::graph::GraphManifest>,
    total_beats: f64,
) {
    let mut existing = std::mem::take(runtimes);
    let mut next_runtimes = Vec::with_capacity(manifests.len());
    for manifest in &manifests {
        let graph_overrides = graph_overrides_for_manifest(manifest, overrides);
        let config = manifest.runtime_config_with_overrides(graph_overrides);
        let next_runtime = if let Some(pos) = existing
            .iter()
            .position(|runtime| runtime.id == manifest.id)
        {
            let mut runtime = existing.swap_remove(pos);
            if runtime.config_compatible(&config) {
                runtime.apply_config_preserving_state(config, total_beats);
                runtime
            } else {
                let mut runtime = crate::graph::GraphRuntime::new_from_config(config);
                runtime.realign(total_beats);
                runtime
            }
        } else {
            let mut runtime = crate::graph::GraphRuntime::new_from_config(config);
            runtime.realign(total_beats);
            runtime
        };
        next_runtimes.push(next_runtime);
    }
    *runtimes = next_runtimes;
    *stored_manifests = manifests;
}
