//! Graph-wide effect-latency accounting and plugin delay compensation.
//!
//! Every effect reports a fixed latency through
//! [`EffectDescriptor::latency_samples`]. Serial chains accumulate by
//! summation; at each summing join point the lower-latency branches are padded
//! by a [`crate::effects::pdc_delay`] node so signals stay phase-aligned:
//!
//! - track primary output edge → its destination (MIX or a bus input),
//! - each track send edge → its destination bus input,
//! - each bus output edge → MIX.
//!
//! Sends tap the track's fx output *before* the primary-edge pad, so every
//! edge gets an independent, always-non-negative pad. Rack-slot chains are
//! included in the per-track total (a rack track's base latency is the max
//! across its slot chains) but the intra-rack join at the voice sum is not
//! yet padded — see `RACK_SLOT_JOIN_UNCOMPENSATED` below.

use super::*;

/// Honest-limitation marker: per-rack-slot pads are computed in the plan but
/// not yet wired to graph nodes, because rack slot chains join at the track
/// voice sum whose wiring is owned by the rack-rebuild signature machinery.
/// Slot chains with latency effects will self-skew inside the rack until a
/// follow-up wires `LatencyPlan::rack_slot_pads`.
pub const RACK_SLOT_JOIN_UNCOMPENSATED: bool = true;

pub(super) use crate::effects::pdc_delay::add_pdc_node;

/// Pure inputs to the latency plan, decoupled from `App` for testability.
#[derive(Clone, Debug, Default)]
pub struct LatencyTopology {
    pub tracks: Vec<TrackLatencyInput>,
    /// (bus id, summed chain latency) for every live bus, in graph order.
    pub buses: Vec<(BusId, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackLatencyInput {
    /// Summed latency of the track-level fx chain (occupied slots only).
    pub chain_latency: u32,
    /// Per-rack-slot chain latencies; empty for non-rack tracks.
    pub rack_slot_latencies: Vec<u32>,
    /// Primary output destination. A route to a bus that is not in
    /// `LatencyTopology::buses` must be pre-normalized to `TrackOutput::Mix`
    /// by the caller (mirroring `connect_delay_output_to`'s fallback).
    pub output: TrackOutput,
    /// Destination buses of active sends (amount > 0).
    pub sends: Vec<BusId>,
}

/// Computed compensation pads, all in samples and all non-negative.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatencyPlan {
    /// Pad on each track's primary output edge.
    pub track_primary_pads: Vec<u32>,
    /// Pad per (track, destination bus) send edge.
    pub send_pads: Vec<Vec<(BusId, u32)>>,
    /// Pad on each bus's output edge into MIX, aligned with the input order.
    pub bus_pads: Vec<(BusId, u32)>,
    /// Pad per (track, rack slot) aligning slot chains at the voice sum.
    /// Computed but not yet applied to graph nodes.
    pub rack_slot_pads: Vec<Vec<u32>>,
    /// Total latency of the fully compensated mix, relative to a
    /// zero-latency source, in samples.
    pub mix_latency: u32,
}

pub fn compute_latency_plan(topology: &LatencyTopology) -> LatencyPlan {
    // Per-track totals: rack base (max slot chain) + track chain.
    let rack_bases: Vec<u32> = topology
        .tracks
        .iter()
        .map(|t| t.rack_slot_latencies.iter().copied().max().unwrap_or(0))
        .collect();
    let track_totals: Vec<u32> = topology
        .tracks
        .iter()
        .zip(&rack_bases)
        .map(|(t, base)| base + t.chain_latency)
        .collect();

    // Bus input alignment: the max latency across every source feeding the
    // bus (primary routes and sends both tap the track fx output).
    let bus_inputs: Vec<u32> = topology
        .buses
        .iter()
        .map(|(bus_id, _)| {
            topology
                .tracks
                .iter()
                .zip(&track_totals)
                .filter(|(t, _)| {
                    t.output == TrackOutput::Bus(*bus_id) || t.sends.contains(bus_id)
                })
                .map(|(_, total)| *total)
                .max()
                .unwrap_or(0)
        })
        .collect();
    let bus_outputs: Vec<u32> = topology
        .buses
        .iter()
        .zip(&bus_inputs)
        .map(|((_, chain), input)| input + chain)
        .collect();

    // MIX alignment: direct tracks and bus outputs.
    let mix_latency = topology
        .tracks
        .iter()
        .zip(&track_totals)
        .filter(|(t, _)| t.output == TrackOutput::Mix)
        .map(|(_, total)| *total)
        .chain(bus_outputs.iter().copied())
        .max()
        .unwrap_or(0);

    let track_primary_pads = topology
        .tracks
        .iter()
        .zip(&track_totals)
        .map(|(t, total)| match t.output {
            TrackOutput::Mix => mix_latency - total,
            TrackOutput::Bus(bus_id) => topology
                .buses
                .iter()
                .position(|(id, _)| *id == bus_id)
                .map(|idx| bus_inputs[idx] - total)
                .unwrap_or(mix_latency - total),
            TrackOutput::None => 0,
        })
        .collect();

    let send_pads = topology
        .tracks
        .iter()
        .zip(&track_totals)
        .map(|(t, total)| {
            t.sends
                .iter()
                .filter_map(|bus_id| {
                    topology
                        .buses
                        .iter()
                        .position(|(id, _)| id == bus_id)
                        .map(|idx| (*bus_id, bus_inputs[idx] - total))
                })
                .collect()
        })
        .collect();

    let bus_pads = topology
        .buses
        .iter()
        .zip(&bus_outputs)
        .map(|((bus_id, _), output)| (*bus_id, mix_latency - output))
        .collect();

    let rack_slot_pads = topology
        .tracks
        .iter()
        .zip(&rack_bases)
        .map(|(t, base)| {
            t.rack_slot_latencies
                .iter()
                .map(|latency| base - latency)
                .collect()
        })
        .collect();

    LatencyPlan {
        track_primary_pads,
        send_pads,
        bus_pads,
        rack_slot_pads,
        mix_latency,
    }
}

/// Sum the latency of every slot that is both occupied and running. `active`
/// must already account for the slot's bypass — see [`slot_is_active`].
fn chain_latency<'a>(
    slots: impl Iterator<Item = (bool, &'a EffectDescriptor, i32)>,
) -> u32 {
    slots
        .filter(|(active, _, _)| *active)
        .map(|(_, desc, node_id)| desc.latency_samples(node_id))
        .sum()
}

/// Whether a slot contributes latency: it must hold a live node and not be
/// bypassed. `enabled_at` reads the slot's stored value for a param index.
fn slot_is_active(
    node_id: i32,
    desc: &EffectDescriptor,
    enabled_at: impl Fn(usize) -> f32,
) -> bool {
    node_id > 0 && desc.enabled_param_idx().is_none_or(|idx| enabled_at(idx) > 0.5)
}

impl App {
    /// Snapshot the current latency topology from project + graph state.
    pub(super) fn latency_topology(&self) -> LatencyTopology {
        let live_buses: Vec<(BusId, u32)> = self
            .graph
            .bus_node_ids
            .iter()
            // The MIX bus is the alignment target itself; its FX chain runs
            // after the final sum and delays everything equally.
            .filter(|nodes| nodes.id != BusId::MIX)
            .filter_map(|nodes| {
                let bus = self.buses.iter().find(|bus| bus.id == nodes.id)?;
                Some((
                    bus.id,
                    chain_latency(
                        bus.effect_slots
                            .iter()
                            .zip(&bus.effect_descriptors)
                            .map(|(slot, desc)| {
                                let node_id = slot.node_id as i32;
                                let active = slot_is_active(node_id, desc, |idx| {
                                    slot.defaults.get(idx).copied().unwrap_or(1.0)
                                });
                                (active, desc, node_id)
                            }),
                    ),
                ))
            })
            .collect();

        let rack_tracks = self.state.pattern.rack_tracks.lock().unwrap();
        let tracks = (0..self.tracks.len())
            .map(|track| {
                let chain = self
                    .state
                    .pattern
                    .effect_chains
                    .get(track)
                    .zip(self.graph.effect_descriptors.get(track))
                    .map(|(slots, descriptors)| {
                        chain_latency(slots.iter().zip(descriptors).map(|(slot, desc)| {
                            let node_id = slot.node_id.load(Ordering::Relaxed) as i32;
                            let active =
                                slot_is_active(node_id, desc, |idx| slot.defaults.get(idx));
                            (active, desc, node_id)
                        }))
                    })
                    .unwrap_or(0);
                let rack_slot_latencies = rack_tracks
                    .get(track)
                    .and_then(Option::as_ref)
                    .map(|rack| {
                        rack.slots
                            .iter()
                            .map(|slot| {
                                chain_latency(
                                    slot.effect_slots
                                        .iter()
                                        .zip(&slot.effect_descriptors)
                                        .map(|(effect, desc)| {
                                            let node_id = effect.node_id as i32;
                                            let active =
                                                slot_is_active(node_id, desc, |idx| {
                                                    effect
                                                        .defaults
                                                        .get(idx)
                                                        .copied()
                                                        .unwrap_or(1.0)
                                                });
                                            (active, desc, node_id)
                                        }),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // This runs from the per-frame reactive tick, so it must
                // tolerate the transient windows during project load and
                // add-track where `track_params` trails `tracks`.
                let Some(params) = self.state.pattern.track_params.get(track) else {
                    return TrackLatencyInput {
                        chain_latency: chain,
                        rack_slot_latencies,
                        output: TrackOutput::None,
                        sends: Vec::new(),
                    };
                };
                let output = match params.output() {
                    TrackOutput::Bus(id) if !live_buses.iter().any(|(bus, _)| *bus == id) => {
                        // Mirror connect_delay_output_to's missing-bus fallback.
                        TrackOutput::Mix
                    }
                    output => output,
                };
                let sends = params
                    .sends()
                    .iter()
                    .filter(|send| send.amount > 0.0)
                    .map(|send| send.destination)
                    .filter(|id| live_buses.iter().any(|(bus, _)| bus == id))
                    .collect();
                TrackLatencyInput {
                    chain_latency: chain,
                    rack_slot_latencies,
                    output,
                    sends,
                }
            })
            .collect();

        LatencyTopology {
            tracks,
            buses: live_buses,
        }
    }

    /// The full (pdc node, pad) target set for the current topology.
    fn latency_pad_targets(&self) -> Vec<(i32, u32)> {
        let plan = compute_latency_plan(&self.latency_topology());
        let mut targets = Vec::new();
        for (track, nodes) in self.graph.track_node_ids.iter().enumerate() {
            targets.push((
                nodes.pdc_id,
                plan.track_primary_pads.get(track).copied().unwrap_or(0),
            ));
            for send_nodes in &nodes.bus_send_ids {
                let pad = plan
                    .send_pads
                    .get(track)
                    .and_then(|sends| {
                        sends
                            .iter()
                            .find(|(bus, _)| *bus == send_nodes.destination)
                            .map(|(_, pad)| *pad)
                    })
                    .unwrap_or(0);
                targets.push((send_nodes.pdc_id, pad));
            }
        }
        for nodes in &self.graph.bus_node_ids {
            let pad = plan
                .bus_pads
                .iter()
                .find(|(bus, _)| *bus == nodes.id)
                .map(|(_, pad)| *pad)
                .unwrap_or(0);
            targets.push((nodes.pdc_id, pad));
        }
        targets.retain(|(node_id, _)| *node_id > 0);
        targets
    }

    /// Recompute the latency plan and push pad updates to every PDC node.
    /// Cheap (pure arithmetic + queued state writes) and change-detecting, so
    /// it is safe to call every frame; also invoked directly after routing and
    /// bus-registry changes.
    pub fn refresh_latency_compensation(&mut self) {
        let lg = self.graph.lg.0;
        if lg.is_null() {
            return;
        }
        let targets = self.latency_pad_targets();
        if targets == self.graph.applied_latency_pads {
            return;
        }

        let max = crate::effects::pdc_delay::PDC_MAX_DELAY_SAMPLES as u32 - 1;
        let _batch = GraphEditBatchGuard::new(lg);
        let mut all_written = true;
        for (node_id, pad) in &targets {
            if *pad > max {
                eprintln!(
                    "latency compensation pad {pad} exceeds PDC capacity {max}; clamping (node {node_id})"
                );
            }
            let value = (*pad).min(max) as f32;
            let written = unsafe {
                crate::audiograph::write_node_state(
                    lg,
                    *node_id,
                    crate::effects::pdc_delay::PDC_PARAM_DELAY,
                    &value,
                    1,
                )
            };
            all_written &= written;
        }
        // Only cache a pad set the graph actually accepted. A dropped write
        // (full edit queue during project load) must stay uncached, or the
        // change detector above would early-return forever and leave that
        // branch uncompensated.
        if all_written {
            self.graph.applied_latency_pads = targets;
        } else {
            self.graph.applied_latency_pads.clear();
        }
    }

    /// Forget the applied-pad cache. Graph teardown deletes the PDC nodes, so
    /// a rebuild that reuses their ids would otherwise match the stale cache
    /// and skip the write, leaving fresh nodes at delay 0.
    pub(in crate::app) fn invalidate_latency_pad_cache(&mut self) {
        self.graph.applied_latency_pads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(chain: u32, output: TrackOutput, sends: &[BusId]) -> TrackLatencyInput {
        TrackLatencyInput {
            chain_latency: chain,
            rack_slot_latencies: Vec::new(),
            output,
            sends: sends.to_vec(),
        }
    }

    #[test]
    fn zero_latency_graph_needs_no_pads() {
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(0, TrackOutput::Mix, &[]),
                track(0, TrackOutput::Mix, &[]),
            ],
            buses: vec![(BusId(1), 0)],
        });
        assert_eq!(plan.mix_latency, 0);
        assert_eq!(plan.track_primary_pads, vec![0, 0]);
        assert_eq!(plan.bus_pads, vec![(BusId(1), 0)]);
    }

    #[test]
    fn serial_chain_latency_accumulates_and_pads_parallel_tracks() {
        // Track 0 has two latency effects in series (2048 + 512); track 1 is
        // dry. Both sum at MIX: track 1 must be padded by the full serial sum.
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(2048 + 512, TrackOutput::Mix, &[]),
                track(0, TrackOutput::Mix, &[]),
            ],
            buses: vec![],
        });
        assert_eq!(plan.mix_latency, 2560);
        assert_eq!(plan.track_primary_pads, vec![0, 2560]);
    }

    #[test]
    fn bus_join_aligns_routed_tracks_and_sends() {
        // Track 0 (latency 2048) routes to bus 1; track 1 (dry) sends to
        // bus 1 and routes to MIX. Bus chain adds 100.
        let bus = BusId(1);
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(2048, TrackOutput::Bus(bus), &[]),
                track(0, TrackOutput::Mix, &[bus]),
            ],
            buses: vec![(bus, 100)],
        });
        // Bus input aligns at 2048; the dry send is padded to match.
        assert_eq!(plan.send_pads[1], vec![(bus, 2048)]);
        assert_eq!(plan.track_primary_pads[0], 0);
        // Mix must wait for the bus (2048 + 100); track 1's direct edge pads
        // to the full mix latency, the bus output needs none.
        assert_eq!(plan.mix_latency, 2148);
        assert_eq!(plan.track_primary_pads[1], 2148);
        assert_eq!(plan.bus_pads, vec![(bus, 0)]);
    }

    #[test]
    fn low_latency_bus_output_is_padded_against_a_hot_direct_track() {
        // Direct track with a Filter Table (2048) vs a dry track through a
        // dry bus: the bus output edge into MIX takes the pad.
        let bus = BusId(2);
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(2048, TrackOutput::Mix, &[]),
                track(0, TrackOutput::Bus(bus), &[]),
            ],
            buses: vec![(bus, 0)],
        });
        assert_eq!(plan.mix_latency, 2048);
        assert_eq!(plan.track_primary_pads, vec![0, 0]);
        assert_eq!(plan.bus_pads, vec![(bus, 2048)]);
    }

    #[test]
    fn rack_slot_chains_align_to_the_widest_slot() {
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![TrackLatencyInput {
                chain_latency: 0,
                rack_slot_latencies: vec![2048, 0, 512],
                output: TrackOutput::Mix,
                sends: Vec::new(),
            }],
            buses: vec![],
        });
        assert_eq!(plan.rack_slot_pads[0], vec![0, 2048, 1536]);
        // The rack base joins the track total.
        assert_eq!(plan.mix_latency, 2048);
        assert_eq!(plan.track_primary_pads, vec![0]);
    }

    use std::os::raw::{c_int, c_void};

    unsafe extern "C" fn impulse_process(
        _inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        let fired = state as *mut f32;
        for ch in 0..2 {
            std::ptr::write_bytes(*out.add(ch), 0, nframes as usize);
        }
        if *fired == 0.0 && nframes > 0 {
            for ch in 0..2 {
                *(*out.add(ch)).add(0) = 1.0;
            }
            *fired = 1.0;
        }
    }

    fn impulse_vtable() -> crate::audiograph::NodeVTable {
        crate::audiograph::NodeVTable {
            process: Some(impulse_process),
            ..crate::audiograph::NodeVTable::default()
        }
    }

    struct EngineGraph {
        lg: *mut crate::audiograph::LiveGraph,
    }

    impl EngineGraph {
        const BLOCK: usize = 64;

        fn new(label: &str) -> Self {
            crate::audiograph::initialize_engine_for_test(Self::BLOCK as c_int, 44_100);
            let label = std::ffi::CString::new(label).unwrap();
            let lg = unsafe {
                crate::audiograph::create_live_graph(32, Self::BLOCK as c_int, label.as_ptr(), 2)
            };
            assert!(!lg.is_null());
            Self { lg }
        }

        fn add_impulse(&self) -> i32 {
            let name = std::ffi::CString::new("impulse").unwrap();
            unsafe {
                crate::audiograph::add_node(
                    self.lg,
                    impulse_vtable(),
                    std::mem::size_of::<f32>(),
                    name.as_ptr(),
                    0,
                    2,
                    std::ptr::null(),
                    0,
                )
            }
        }

        fn set_pdc_delay(&self, node_id: i32, delay: f32) {
            unsafe {
                assert!(crate::audiograph::write_node_state(
                    self.lg,
                    node_id,
                    crate::effects::pdc_delay::PDC_PARAM_DELAY,
                    &delay,
                    1,
                ));
            }
        }

        /// Render `blocks` blocks and return channel 0 deinterleaved.
        fn render_channel0(&self, blocks: usize) -> Vec<f32> {
            let mut collected = Vec::new();
            let mut output = vec![0.0f32; Self::BLOCK * 2];
            for _ in 0..blocks {
                unsafe {
                    crate::audiograph::process_next_block(
                        self.lg,
                        output.as_mut_ptr(),
                        Self::BLOCK as c_int,
                    );
                }
                collected.extend(output.iter().step_by(2).copied());
            }
            collected
        }
    }

    impl Drop for EngineGraph {
        fn drop(&mut self) {
            unsafe { crate::audiograph::destroy_live_graph(self.lg) };
        }
    }

    fn spikes(samples: &[f32]) -> Vec<(usize, f32)> {
        samples
            .iter()
            .enumerate()
            .filter(|(_, v)| v.abs() > 1e-6)
            .map(|(i, v)| (i, *v))
            .collect()
    }

    /// End-to-end through the real C engine: an impulse split into two
    /// parallel branches, one carrying a latency-300 "effect" (a PDC node
    /// standing in for any fixed-latency processor), summed at the DAC.
    /// Without compensation the branches arrive 300 samples apart; with the
    /// planner's pad on the dry branch they sum into one aligned spike.
    #[test]
    fn parallel_branches_sum_in_phase_after_compensation() {
        const EFFECT_LATENCY: u32 = 300;
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(EFFECT_LATENCY, TrackOutput::Mix, &[]),
                track(0, TrackOutput::Mix, &[]),
            ],
            buses: vec![],
        });
        assert_eq!(plan.track_primary_pads, vec![0, EFFECT_LATENCY]);

        for (compensate, expected) in [
            (false, vec![(0usize, 1.0f32), (EFFECT_LATENCY as usize, 1.0)]),
            (true, vec![(EFFECT_LATENCY as usize, 2.0)]),
        ] {
            let graph = EngineGraph::new(if compensate {
                "pdc-aligned"
            } else {
                "pdc-misaligned"
            });
            let source = graph.add_impulse();
            let effect = add_pdc_node(graph.lg, "branch_effect");
            let pad = add_pdc_node(graph.lg, "branch_pad");
            unsafe {
                crate::audiograph::graph_connect(graph.lg, source, 0, effect, 0);
                crate::audiograph::graph_connect(graph.lg, source, 1, effect, 1);
                crate::audiograph::graph_connect(graph.lg, source, 0, pad, 0);
                crate::audiograph::graph_connect(graph.lg, source, 1, pad, 1);
                // Both branches sum at the DAC's channel inputs (node 0).
                crate::audiograph::graph_connect(graph.lg, effect, 0, 0, 0);
                crate::audiograph::graph_connect(graph.lg, effect, 1, 0, 1);
                crate::audiograph::graph_connect(graph.lg, pad, 0, 0, 0);
                crate::audiograph::graph_connect(graph.lg, pad, 1, 0, 1);
            }
            graph.set_pdc_delay(effect, EFFECT_LATENCY as f32);
            if compensate {
                graph.set_pdc_delay(pad, plan.track_primary_pads[1] as f32);
            }
            let rendered = graph.render_channel0(8);
            assert_eq!(
                spikes(&rendered),
                expected,
                "compensate={compensate} rendered wrong spike pattern"
            );
        }
    }

    #[test]
    fn filter_table_descriptor_reports_stft_latency() {
        // Latency is keyed by descriptor name (dgen builtins take their
        // descriptor name from the compile manifest, which uses NAME), with
        // the per-node engine deciding between STFT latency and zero.
        let node_id = i32::MAX - 7; // unlikely to collide with live test graphs
        let mut desc = EffectDescriptor::empty_custom_slot();
        desc.name = crate::effects::filter_table::NAME.to_string();
        assert_eq!(
            desc.latency_samples(node_id),
            crate::effects::filter_table::N as u32
        );
        crate::effects::filter_table::record_engine(
            node_id,
            crate::effects::filter_table::TableEngine::Causal,
        );
        assert_eq!(desc.latency_samples(node_id), 0);
        crate::effects::filter_table::clear_instance(node_id);
        desc.name = "Delay".to_string();
        assert_eq!(desc.latency_samples(node_id), 0);
    }

    #[test]
    fn bypassed_slot_contributes_no_latency() {
        // The DGen wrapper's bypass is a bit-exact passthrough with no delay
        // line, so a disabled Filter Table must drop out of the plan — leaving
        // it in would keep padding parallel branches against latency the
        // signal no longer accrues.
        let _registry = crate::effects::filter_table::tests::registry_lock();
        let node_id = i32::MAX - 11;
        let mut desc = EffectDescriptor::empty_custom_slot();
        desc.name = crate::effects::filter_table::NAME.to_string();
        desc.params = vec![EffectDescriptor::enabled_param(0, 1.0)];
        assert_eq!(desc.enabled_param_idx(), Some(0));

        let latency_when = |enabled: f32| {
            let active = slot_is_active(node_id, &desc, |_| enabled);
            chain_latency(std::iter::once((active, &desc, node_id)))
        };
        assert_eq!(latency_when(1.0), crate::effects::filter_table::N as u32);
        assert_eq!(latency_when(0.0), 0);
        // An empty slot is inactive whatever the stored value says.
        assert!(!slot_is_active(0, &desc, |_| 1.0));
        crate::effects::filter_table::clear_instance(node_id);
    }

    #[test]
    fn slot_without_an_enabled_param_stays_active() {
        let node_id = i32::MAX - 13;
        let mut desc = EffectDescriptor::empty_custom_slot();
        desc.params.clear();
        desc.name = "Delay".to_string();
        assert_eq!(desc.enabled_param_idx(), None);
        assert!(slot_is_active(node_id, &desc, |_| 0.0));
    }

    #[test]
    fn unrouted_track_never_pads() {
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![
                track(0, TrackOutput::None, &[]),
                track(2048, TrackOutput::Mix, &[]),
            ],
            buses: vec![],
        });
        assert_eq!(plan.track_primary_pads, vec![0, 0]);
    }
}
