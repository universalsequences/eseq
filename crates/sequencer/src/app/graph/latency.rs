//! Graph-wide effect-latency accounting and plugin delay compensation.
//!
//! Every effect reports a fixed latency through
//! [`EffectDescriptor::latency_samples`]. Serial chains accumulate by
//! summation; at each summing join point the lower-latency branches are padded
//! by a [`crate::effects::pdc_delay`] node so signals stay phase-aligned:
//!
//! - track primary output edge → its destination (MIX or a bus input),
//! - each track send edge → its destination bus input,
//! - each bus output edge → its destination (MIX or another bus input).
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
    /// Bus output edges. An absent edge terminates at the master mix.
    pub bus_destinations: Vec<(BusId, BusId)>,
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
    /// Pad on each bus's output edge, aligned with the input order.
    pub bus_pads: Vec<(BusId, u32)>,
    /// Pad per (track, rack slot) aligning slot chains at the voice sum.
    /// Computed but not yet applied to graph nodes.
    pub rack_slot_pads: Vec<Vec<u32>>,
    /// Total latency of the fully compensated mix, relative to a
    /// zero-latency source, in samples.
    pub mix_latency: u32,
}

impl LatencyPlan {
    /// Zero-gain sends can enter/leave the planner without changing any delay.
    /// Compare effective compensation, not the presence of zero-valued entries.
    pub(crate) fn same_compensation(&self, other: &Self) -> bool {
        fn sends(plan: &LatencyPlan) -> Vec<(usize, BusId, u32)> {
            let mut result: Vec<_> = plan.send_pads.iter().enumerate()
                .flat_map(|(track, pads)| pads.iter().filter(|(_, pad)| *pad != 0)
                    .map(move |(bus, pad)| (track, *bus, *pad))).collect();
            result.sort_by_key(|(track, bus, pad)| (*track, bus.0, *pad));
            result
        }
        self.mix_latency == other.mix_latency
            && self.track_primary_pads == other.track_primary_pads
            && self.bus_pads == other.bus_pads
            && self.rack_slot_pads == other.rack_slot_pads
            && sends(self) == sends(other)
    }

    pub(crate) fn validate_bounce(&self) -> Result<(), String> {
        if RACK_SLOT_JOIN_UNCOMPENSATED
            && self.rack_slot_pads.iter().flatten().any(|pad| *pad != 0)
        {
            return Err("Export requires intra-rack delay compensation that is not installed".into());
        }
        let max = crate::effects::pdc_delay::PDC_MAX_DELAY_SAMPLES as u32 - 1;
        if self.track_primary_pads.iter().copied()
            .chain(self.send_pads.iter().flatten().map(|(_, pad)| *pad))
            .chain(self.bus_pads.iter().map(|(_, pad)| *pad))
            .chain(self.rack_slot_pads.iter().flatten().copied())
            .any(|pad| pad > max)
        {
            return Err(format!("Export delay compensation exceeds the supported {max}-frame pad"));
        }
        Ok(())
    }
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
    let mut bus_inputs: Vec<u32> = topology
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
    let destinations: Vec<Option<usize>> = topology.buses.iter().map(|(id, _)| {
        topology.bus_destinations.iter().find(|(source, _)| source == id)
            .and_then(|(_, destination)| topology.buses.iter().position(|(id, _)| id == destination))
    }).collect();
    let mut incoming = vec![0usize; topology.buses.len()];
    for target in destinations.iter().flatten() { incoming[*target] += 1; }
    let mut ready: std::collections::VecDeque<_> = incoming.iter().enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index)).collect();
    let mut bus_outputs = vec![0; topology.buses.len()];
    let mut visited = 0;
    while let Some(index) = ready.pop_front() {
        visited += 1;
        bus_outputs[index] = bus_inputs[index] + topology.buses[index].1;
        if let Some(target) = destinations[index] {
            bus_inputs[target] = bus_inputs[target].max(bus_outputs[index]);
            incoming[target] -= 1;
            if incoming[target] == 0 { ready.push_back(target); }
        }
    }
    assert_eq!(visited, topology.buses.len(), "latency topology contains a bus cycle");

    // MIX alignment: direct tracks and bus outputs.
    let mix_latency = topology
        .tracks
        .iter()
        .zip(&track_totals)
        .filter(|(t, _)| t.output == TrackOutput::Mix)
        .map(|(_, total)| *total)
        .chain(bus_outputs.iter().enumerate()
            .filter(|(index, _)| destinations[*index].is_none()).map(|(_, output)| *output))
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
        .enumerate()
        .map(|(index, ((bus_id, _), output))| {
            let target_latency = destinations[index].map(|target| bus_inputs[target]).unwrap_or(mix_latency);
            (*bus_id, target_latency - output)
        })
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
    /// Install and acknowledge the initial compensation without consuming
    /// frame zero. The returned integer latency is suitable for BouncePlan.
    ///
    /// # Safety
    /// Call only on the isolated worker's graph with exclusive render ownership;
    /// this must never run concurrently with a live audio callback.
    pub(crate) unsafe fn prepare_bounce_latency(&mut self) -> Result<LatencyPlan, String> {
        let lg = self.graph.lg.0;
        if lg.is_null() {
            return Err("Export has no prepared audio graph".into());
        }
        let mut plan = compute_latency_plan(&self.latency_topology());
        // Master inserts follow the compensated join, so their latency belongs
        // in the export trim without adding it to any upstream branch pad.
        if let Some(master) = self.buses.iter().find(|bus| bus.id == BusId::MIX) {
            let master_latency = chain_latency(master.effect_slots.iter()
                .zip(&master.effect_descriptors).map(|(slot, desc)| {
                    let active = slot_is_active(slot.node_id as i32, desc, |idx| {
                        slot.defaults.get(idx).copied().unwrap_or(1.0)
                    });
                    (active, desc, slot.node_id as i32)
                }));
            plan.mix_latency = plan.mix_latency.checked_add(master_latency)
                .ok_or_else(|| "Export latency exceeds the supported sample clock".to_string())?;
        }
        plan.validate_bounce()?;
        for (track, pad) in plan.track_primary_pads.iter().enumerate() {
            let nodes = self.graph.track_node_ids.get(track)
                .ok_or_else(|| format!("Export track {track} has no graph nodes"))?;
            if *pad > 0 && nodes.pdc_id <= 0 {
                return Err(format!("Export track {track} has no delay compensation node"));
            }
            for (destination, pad) in &plan.send_pads[track] {
                if *pad > 0 && !nodes.bus_send_ids.iter()
                    .any(|send| send.destination == *destination && send.pdc_id > 0)
                {
                    return Err(format!("Export track {track} send to {destination:?} has no delay compensation node"));
                }
            }
        }
        for (bus, pad) in &plan.bus_pads {
            if *pad > 0 && !self.graph.bus_node_ids.iter()
                .any(|nodes| nodes.id == *bus && nodes.pdc_id > 0)
            {
                return Err(format!("Export bus {bus:?} has no delay compensation node"));
            }
        }
        self.refresh_latency_compensation();
        if !crate::audiograph::prepare_graph_for_render(lg) {
            return Err("Export graph could not apply its initial delay compensation".into());
        }
        let submissions = crate::audiograph::graph_control_submission_failures(lg);
        let deliveries = crate::audiograph::graph_edit_delivery_failures(lg);
        if submissions != 0 || deliveries != 0 {
            return Err(format!("Export graph initialization failed: {submissions} rejected submissions, {deliveries} failed edits"));
        }
        Ok(plan)
    }

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
            bus_destinations: self.buses.iter().filter_map(|bus| {
                bus.output.destination().map(|target| (bus.id, BusId(target)))
            }).collect(),
            buses: live_buses,
        }
    }

    /// The full (pdc node, pad) target set for the current topology, plus the
    /// plan's total mix latency.
    fn latency_pad_targets(&self) -> (Vec<(i32, u32)>, u32) {
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
        (targets, plan.mix_latency)
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
        let (targets, mix_latency) = self.latency_pad_targets();
        // Publish the total before the pad change-detection below: a plan can
        // move the mix total while leaving every pad identical. Removing the
        // only latent effect from a single-track project takes mix_latency
        // 2048 -> 0 with the one pad at 0 throughout, and recording would
        // otherwise keep compensating for latency that is gone.
        self.state
            .set_pdc_latency_seconds(mix_latency as f32 / self.graph.sample_rate.max(1) as f32);
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

    #[test]
    fn chained_bus_joins_compensate_each_destination() {
        let a = BusId(1);
        let b = BusId(2);
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![track(10, TrackOutput::Bus(a), &[]),
                track(100, TrackOutput::Bus(b), &[]), track(200, TrackOutput::Mix, &[a])],
            buses: vec![(b, 30), (a, 20)],
            bus_destinations: vec![(a, b)],
        });
        assert_eq!(plan.mix_latency, 250);
        assert_eq!(plan.track_primary_pads, vec![190, 120, 50]);
        assert_eq!(plan.bus_pads, vec![(b, 0), (a, 0)]);
        assert_eq!(plan.send_pads[2], vec![(a, 0)]);
        let plan = compute_latency_plan(&LatencyTopology {
            tracks: vec![track(10, TrackOutput::Bus(a), &[]),
                track(100, TrackOutput::Bus(b), &[]), track(400, TrackOutput::Mix, &[])],
            buses: vec![(b, 30), (a, 20)],
            bus_destinations: vec![(a, b)],
        });
        assert_eq!(plan.mix_latency, 400);
        assert_eq!(plan.bus_pads, vec![(b, 270), (a, 70)]);
        assert_eq!(plan.track_primary_pads, vec![0, 0, 0]);
    }

    fn track(chain: u32, output: TrackOutput, sends: &[BusId]) -> TrackLatencyInput {
        TrackLatencyInput {
            chain_latency: chain,
            rack_slot_latencies: Vec::new(),
            output,
            sends: sends.to_vec(),
        }
    }

    #[test]
    fn bounce_compensation_ignores_only_zero_delay_send_membership() {
        let mut a = LatencyPlan::default();
        a.send_pads = vec![vec![(BusId(1), 0)]];
        let mut b = a.clone();
        b.send_pads[0].clear();
        assert!(a.same_compensation(&b));
        a.send_pads[0][0].1 = 12;
        assert!(!a.same_compensation(&b));
        b = a.clone();
        b.mix_latency += 1;
        assert!(!a.same_compensation(&b));
    }

    #[test]
    fn bounce_rejects_uninstalled_rack_pads_and_oversize_compensation() {
        let mut topology = LatencyTopology {
            bus_destinations: Vec::new(),
            tracks: vec![TrackLatencyInput {
                rack_slot_latencies: vec![0, 300], output: TrackOutput::Mix,
                ..Default::default()
            }], buses: Vec::new(),
        };
        assert!(compute_latency_plan(&topology).validate_bounce().unwrap_err().contains("intra-rack"));
        topology.tracks[0].rack_slot_latencies = vec![300, 300];
        assert!(compute_latency_plan(&topology).validate_bounce().is_ok());
        let too_large = crate::effects::pdc_delay::PDC_MAX_DELAY_SAMPLES as u32;
        topology.tracks = vec![track(too_large, TrackOutput::Mix, &[])];
        assert!(compute_latency_plan(&topology).validate_bounce().is_ok(),
            "total serial latency is not limited by the size of an alignment pad");
        topology.tracks.push(track(0, TrackOutput::Mix, &[]));
        assert!(compute_latency_plan(&topology).validate_bounce().unwrap_err().contains("supported"));
        let mut plan = LatencyPlan::default();
        plan.send_pads = vec![vec![(BusId(1), too_large)]];
        assert!(plan.validate_bounce().is_err());
        plan.send_pads.clear();
        plan.bus_pads = vec![(BusId(1), too_large)];
        assert!(plan.validate_bounce().is_err());
    }

    #[test]
    fn zero_latency_graph_needs_no_pads() {
        let plan = compute_latency_plan(&LatencyTopology {
            bus_destinations: Vec::new(),
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
            bus_destinations: Vec::new(),
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
            bus_destinations: Vec::new(),
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
            bus_destinations: Vec::new(),
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
            bus_destinations: Vec::new(),
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

    #[test]
    fn graph_preparation_acknowledges_initialization_without_advancing_dsp() {
        let graph = EngineGraph::new("prepare-without-render");
        let serial;
        unsafe {
            crate::audiograph::begin_graph_edit_batch(graph.lg);
            serial = crate::audiograph::graph_edit_current_batch_serial(graph.lg);
            let impulse = graph.add_impulse();
            connect_stereo_pair(graph.lg, impulse, 0);
            assert!(!crate::audiograph::prepare_graph_for_render(graph.lg),
                "an open batch must not be applied");
            crate::audiograph::end_graph_edit_batch(graph.lg);
            assert!(crate::audiograph::graph_edit_applied_batch_serial(graph.lg) < serial);
            assert!(crate::audiograph::prepare_graph_for_render(graph.lg));
            assert!(crate::audiograph::graph_edit_applied_batch_serial(graph.lg) >= serial);
            assert!(crate::audiograph::prepare_graph_for_render(graph.lg));
        }
        assert_eq!(spikes(&graph.render_channel0(2)), vec![(0, 1.0)]);
    }

    #[test]
    fn bounce_trims_installed_pdc_once_and_keeps_exact_tail_length() {
        let graph = EngineGraph::new("bounce-pdc-interval");
        let latency = compute_latency_plan(&LatencyTopology {
            bus_destinations: Vec::new(),
            tracks: vec![track(300, TrackOutput::Mix, &[]), track(0, TrackOutput::Mix, &[])],
            buses: Vec::new(),
        });
        latency.validate_bounce().unwrap();
        let source = graph.add_impulse();
        let effect = add_pdc_node(graph.lg, "latency-effect");
        let pad = add_pdc_node(graph.lg, "compensated-dry");
        unsafe {
            connect_stereo_pair(graph.lg, source, effect);
            connect_stereo_pair(graph.lg, source, pad);
            connect_stereo_pair(graph.lg, effect, 0);
            connect_stereo_pair(graph.lg, pad, 0);
        }
        graph.set_pdc_delay(effect, 300.0);
        graph.set_pdc_delay(pad, latency.track_primary_pads[1] as f32);
        assert!(unsafe { crate::audiograph::prepare_graph_for_render(graph.lg) });
        let plan = crate::bounce::BouncePlan::new(44_100, EngineGraph::BLOCK, 120,
            0.03, None, latency.mix_latency, 0.004).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compensated.wav");
        crate::bounce::render_to_wav(&plan, &path, crate::bounce::Publication::CreateNew,
            &crate::bounce::BounceCancellation::default(), |_, output| {
                unsafe { crate::audiograph::process_next_block(graph.lg, output.as_mut_ptr(), EngineGraph::BLOCK as i32); }
                Ok(())
            }, |_| {}).unwrap();
        let reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), 662 + 177);
        let samples: Vec<f32> = reader.into_samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(&samples[..2], &[2.0, 2.0]);
        assert!(samples[2..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn bounce_preparation_installs_real_app_pads_and_rejects_missing_nodes() {
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        let lg = engine.lg_ptr.0;
        let mut app = App::new(engine.state, engine.lg_ptr, engine.sample_rate,
            engine.buses, engine.master_recorder, engine.keyboard_tx);
        app.graph_controller().add_blank_sampler_track().unwrap();
        app.graph_controller().add_blank_sampler_track().unwrap();
        app.add_builtin_effect_sync(0, crate::effects::filter_table::NAME).unwrap();
        let plan = unsafe { app.prepare_bounce_latency() }.unwrap();
        assert!(plan.mix_latency > 0);
        assert_eq!(plan.track_primary_pads[1], plan.mix_latency);
        assert!(app.graph.applied_latency_pads.contains(&(app.graph.track_node_ids[1].pdc_id, plan.mix_latency)));
        let saved = app.graph.track_node_ids[1].pdc_id;
        app.graph.track_node_ids[1].pdc_id = 0;
        assert!(unsafe { app.prepare_bounce_latency() }.unwrap_err().contains("no delay compensation node"));
        app.graph.track_node_ids[1].pdc_id = saved;
        let master = app.buses.iter().position(|bus| bus.id == BusId::MIX).unwrap();
        app.add_builtin_bus_effect_sync(master, crate::effects::filter_table::NAME).unwrap();
        let with_master = unsafe { app.prepare_bounce_latency() }.unwrap();
        assert_eq!(with_master.mix_latency, plan.mix_latency * 2);
        assert_eq!(with_master.track_primary_pads, plan.track_primary_pads,
            "master latency belongs in the trim, not in upstream branch pads");
        unsafe {
            crate::audiograph::engine_stop_workers();
            crate::audiograph::destroy_live_graph(lg);
        }
    }

    #[test]
    fn queued_disconnect_accepts_absent_links_but_rejects_invalid_ports() {
        let graph = EngineGraph::new("disconnect-result");
        let first = graph.add_impulse();
        let second = graph.add_impulse();
        unsafe {
            connect_stereo_pair(graph.lg, first, 0);
            connect_stereo_pair(graph.lg, second, 0);
            assert!(crate::audiograph::prepare_graph_for_render(graph.lg));
            for _ in 0..2 {
                assert!(crate::audiograph::graph_disconnect(graph.lg, first, 0, 0, 0));
                assert!(crate::audiograph::graph_disconnect(graph.lg, first, 1, 0, 1));
                assert!(crate::audiograph::prepare_graph_for_render(graph.lg));
            }
            assert_eq!(crate::audiograph::graph_edit_delivery_failures(graph.lg), 0);
        }
        assert_eq!(spikes(&graph.render_channel0(1)), vec![(0, 1.0)],
            "the other summing branch must survive repeated disconnects");
        unsafe {
            assert!(crate::audiograph::graph_disconnect(graph.lg, second, 5, 0, 0));
            assert!(!crate::audiograph::prepare_graph_for_render(graph.lg));
            assert_eq!(crate::audiograph::graph_edit_delivery_failures(graph.lg), 1);
        }
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
            bus_destinations: Vec::new(),
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
    fn a_lone_latent_track_moves_the_mix_total_without_moving_any_pad() {
        // Why `refresh_latency_compensation` must publish the mix total
        // BEFORE its pad change-detection: on a single-track project the pad
        // is 0 whether or not the effect is there, so a publish gated on the
        // pad set changing would never fire, and recording would keep
        // compensating for latency that had been removed.
        let latent = compute_latency_plan(&LatencyTopology {
            bus_destinations: Vec::new(),
            tracks: vec![track(2048, TrackOutput::Mix, &[])],
            buses: vec![],
        });
        let dry = compute_latency_plan(&LatencyTopology {
            bus_destinations: Vec::new(),
            tracks: vec![track(0, TrackOutput::Mix, &[])],
            buses: vec![],
        });
        assert_eq!(latent.track_primary_pads, dry.track_primary_pads);
        assert_eq!(latent.mix_latency, 2048);
        assert_eq!(dry.mix_latency, 0);
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
            bus_destinations: Vec::new(),
            tracks: vec![
                track(0, TrackOutput::None, &[]),
                track(2048, TrackOutput::Mix, &[]),
            ],
            buses: vec![],
        });
        assert_eq!(plan.track_primary_pads, vec![0, 0]);
    }
}
