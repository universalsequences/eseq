//! One compensation schedule for every routing choice in an arrangement.
//!
//! A primary track/bus output owns one delay line even when its destination
//! changes. All of that output's possible destinations must therefore have
//! the same input arrival time. Sends own independent delay lines. Solve
//! those equalities and the bus-chain latency constraints before frame zero;
//! never move a delay tap (and lose/repeat its history) at a scene boundary.

use super::*;
use super::latency::{LatencyPlan, LatencyTopology};

pub(crate) struct FixedBounceLatency {
    sources: LatencyTopology,
    track_outputs: Vec<Vec<TrackOutput>>,
    bus_outputs: Vec<Vec<BusId>>,
    pub sends: Vec<Vec<BusId>>,
    pub plan: LatencyPlan,
}

fn insert_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) { values.push(value); }
}

fn normalize_output(output: &TrackOutput, topology: &LatencyTopology) -> TrackOutput {
    match output {
        TrackOutput::Bus(id) if !topology.buses.iter().any(|(bus, _)| bus == id) => TrackOutput::Mix,
        output => output.clone(),
    }
}

impl FixedBounceLatency {
    pub fn validate(&self, current: &LatencyTopology) -> Result<(), String> {
        if current.buses != self.sources.buses || current.tracks.len() != self.sources.tracks.len()
            || current.tracks.iter().zip(&self.sources.tracks).any(|(a, b)| {
                a.chain_latency != b.chain_latency || a.rack_slot_latencies != b.rack_slot_latencies
            })
        {
            return Err("Arrangement changes effect processing latency; export requires fixed processing latency".into());
        }
        for (index, track) in current.tracks.iter().enumerate() {
            if !self.track_outputs[index].contains(&track.output)
                || track.sends.iter().any(|bus| !self.sends[index].contains(bus))
            {
                return Err(format!("Export track {index} uses a route absent from arrangement preflight"));
            }
        }
        for (index, (bus, _)) in current.buses.iter().enumerate() {
            let destination = current.bus_destinations.iter().find(|(source, _)| source == bus)
                .map(|(_, target)| *target).unwrap_or(BusId::MIX);
            if !self.bus_outputs[index].contains(&destination) {
                return Err(format!("Export bus {bus:?} uses a route absent from arrangement preflight"));
            }
        }
        Ok(())
    }
}

impl App {
    pub(crate) fn prepare_bounce_routing(&mut self, song: &crate::sequencer::RuntimeSong, end_beat: f64) -> Result<(), String> {
        let sources = self.latency_topology();
        let mut track_outputs: Vec<_> = sources.tracks.iter().map(|track| vec![track.output.clone()]).collect();
        let mut sends: Vec<_> = sources.tracks.iter().map(|track| track.sends.clone()).collect();
        let mut bus_outputs = vec![Vec::new(); sources.buses.len()];
        let default_buses = self.capture_bus_pattern_snapshot();
        let mut scene = self.state.current_scene_index();
        for row in song.rows.iter().filter(|row| row.start_beat < end_beat) {
            for (index, track) in row.scheduler_snapshot.tracks.iter().enumerate() {
                // Silent lanes contain scheduler placeholders, not recalled
                // mixer settings. The control mirror holds the prior sound
                // and routing so releases and effects can keep ringing.
                if track.scene_silenced { continue; }
                insert_unique(&mut track_outputs[index], normalize_output(&track.params.output, &sources));
                for send in track.params.sends.iter()
                    .chain(track.steps.iter().flat_map(|step| &step.track_send_plocks))
                {
                    if sources.buses.iter().any(|(id, _)| *id == send.destination) {
                        insert_unique(&mut sends[index], send.destination);
                    }
                }
            }
            scene = row.scene.unwrap_or(scene);
            let buses = self.state.bus_pattern_snapshot_or_default(scene, &default_buses);
            for (index, (id, _)) in sources.buses.iter().enumerate() {
                let target = buses.iter().find(|bus| bus.id == *id)
                    .and_then(|bus| bus.output.destination()).map(BusId)
                    .filter(|target| sources.buses.iter().any(|(id, _)| id == target))
                    .unwrap_or(BusId::MIX);
                insert_unique(&mut bus_outputs[index], target);
            }
        }
        let plan = fixed_plan(&sources, &track_outputs, &sends, &bus_outputs)?;
        plan.validate_bounce()?;
        self.graph.bounce_latency = Some(FixedBounceLatency {
            sources, track_outputs, bus_outputs, sends, plan,
        });
        for track in 0..self.tracks.len() {
            self.graph_controller().apply_track_bus_sends(track);
        }
        Ok(())
    }
}

fn fixed_plan(
    topology: &LatencyTopology,
    track_outputs: &[Vec<TrackOutput>],
    sends: &[Vec<BusId>],
    bus_outputs: &[Vec<BusId>],
) -> Result<LatencyPlan, String> {
    // Join 0 is MIX; every other join is the input of one bus.
    let join = |bus: BusId| topology.buses.iter().position(|(id, _)| *id == bus)
        .map(|index| index + 1).unwrap_or(0);
    let primary = |output: &TrackOutput| match output {
        TrackOutput::Mix => Some(0),
        TrackOutput::Bus(bus) => Some(join(*bus)),
        TrackOutput::None => None,
    };
    let count = topology.buses.len() + 1;
    let mut group: Vec<usize> = (0..count).collect();
    for destinations in track_outputs.iter().map(|outputs| outputs.iter().filter_map(primary).collect::<Vec<_>>())
        .chain(bus_outputs.iter().map(|outputs| outputs.iter().map(|id| join(*id)).collect()))
    {
        if let Some(first) = destinations.first() {
            for destination in &destinations[1..] {
                let (from, to) = (group[*destination], group[*first]);
                for value in &mut group { if *value == from { *value = to; } }
            }
        }
    }
    let rack_bases: Vec<_> = topology.tracks.iter()
        .map(|track| track.rack_slot_latencies.iter().copied().max().unwrap_or(0)).collect();
    let totals: Vec<_> = topology.tracks.iter().zip(&rack_bases)
        .map(|(track, rack)| track.chain_latency.checked_add(*rack)
            .ok_or_else(|| "Export latency exceeds the supported sample clock".to_string()))
        .collect::<Result<_, _>>()?;
    let mut arrivals = vec![0u32; count];
    for (track, total) in totals.iter().enumerate() {
        for target in track_outputs[track].iter().filter_map(primary)
            .chain(sends[track].iter().map(|bus| join(*bus)))
        {
            arrivals[group[target]] = arrivals[group[target]].max(*total);
        }
    }
    // Longest-path relaxation also handles zero-latency routing cycles across
    // DIFFERENT scenes. A positive cycle cannot have a finite fixed schedule.
    for pass in 0..count {
        let mut changed = false;
        for (index, (_, latency)) in topology.buses.iter().enumerate() {
            let output = arrivals[group[index + 1]].checked_add(*latency)
                .ok_or("Export latency exceeds the supported sample clock")?;
            for target in &bus_outputs[index] {
                let value = &mut arrivals[group[join(*target)]];
                if *value < output { *value = output; changed = true; }
            }
        }
        if !changed { break; }
        if pass + 1 == count {
            return Err("Arrangement routing cannot preserve fixed delay compensation across a latency-bearing bus; use consistent bus outputs across scenes".into());
        }
    }
    let arrival = |target: usize| arrivals[group[target]];
    Ok(LatencyPlan {
        track_primary_pads: totals.iter().enumerate().map(|(track, total)| {
            track_outputs[track].iter().find_map(primary).map(|target| arrival(target) - total).unwrap_or(0)
        }).collect(),
        send_pads: sends.iter().zip(&totals).map(|(sends, total)| {
            sends.iter().map(|bus| (*bus, arrival(join(*bus)) - total)).collect()
        }).collect(),
        bus_pads: topology.buses.iter().enumerate().map(|(index, (bus, latency))| {
            (*bus, arrival(join(bus_outputs[index][0])) - arrival(index + 1) - latency)
        }).collect(),
        rack_slot_pads: topology.tracks.iter().zip(rack_bases).map(|(track, base)| {
            track.rack_slot_latencies.iter().map(|latency| base - latency).collect()
        }).collect(),
        mix_latency: arrival(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::latency::TrackLatencyInput;

    fn topology() -> LatencyTopology {
        LatencyTopology {
            tracks: vec![TrackLatencyInput { output: TrackOutput::Mix, ..Default::default() },
                TrackLatencyInput { chain_latency: 300, output: TrackOutput::Mix, ..Default::default() }],
            buses: vec![(BusId(1), 0), (BusId(2), 0)],
            bus_destinations: Vec::new(),
        }
    }

    #[test]
    fn alternate_primary_and_bus_routes_keep_every_path_aligned() {
        let topology = topology();
        let plan = fixed_plan(&topology,
            &[vec![TrackOutput::Mix, TrackOutput::Bus(BusId(1))], vec![TrackOutput::Mix]],
            &[vec![BusId(2)], vec![]],
            &[vec![BusId::MIX, BusId(2)], vec![BusId::MIX]],
        ).unwrap();
        assert_eq!(plan.mix_latency, 300);
        assert_eq!(plan.track_primary_pads, vec![300, 0]);
        assert_eq!(plan.send_pads, vec![vec![(BusId(2), 300)], vec![]]);
        assert_eq!(plan.bus_pads, vec![(BusId(1), 0), (BusId(2), 0)]);
    }

    #[test]
    fn fixed_routing_rejects_positive_latency_cycles_but_allows_zero_cycles() {
        let mut topology = topology();
        let outputs = [vec![TrackOutput::Mix], vec![TrackOutput::Mix]];
        let buses = [vec![BusId::MIX, BusId(2)], vec![BusId::MIX, BusId(1)]];
        assert!(fixed_plan(&topology, &outputs, &[vec![], vec![]], &buses).is_ok());
        topology.buses[1].1 = 10;
        assert!(fixed_plan(&topology, &outputs, &[vec![], vec![]], &buses)
            .unwrap_err().contains("latency-bearing bus"));
    }

    #[test]
    fn fixed_routing_validation_rejects_changed_effect_latency_and_unknown_routes() {
        let sources = topology();
        let fixed = FixedBounceLatency {
            sources: sources.clone(),
            track_outputs: vec![vec![TrackOutput::Mix], vec![TrackOutput::Mix]],
            sends: vec![vec![BusId(1)], vec![]],
            bus_outputs: vec![vec![BusId::MIX], vec![BusId::MIX]],
            plan: LatencyPlan::default(),
        };
        fixed.validate(&sources).unwrap();
        let mut changed = sources.clone();
        changed.tracks[0].chain_latency = 1;
        assert!(fixed.validate(&changed).unwrap_err().contains("processing latency"));
        changed = sources.clone();
        changed.buses[0].1 = 1;
        assert!(fixed.validate(&changed).unwrap_err().contains("processing latency"));
        changed = sources.clone();
        changed.tracks[0].sends.push(BusId(2));
        assert!(fixed.validate(&changed).unwrap_err().contains("route absent"));
        changed = sources.clone();
        changed.tracks[0].output = TrackOutput::Bus(BusId(1));
        assert!(fixed.validate(&changed).unwrap_err().contains("route absent"));
        changed = sources;
        changed.bus_destinations.push((BusId(1), BusId(2)));
        assert!(fixed.validate(&changed).unwrap_err().contains("route absent"));
    }
}
