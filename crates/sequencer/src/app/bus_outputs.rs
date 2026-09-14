use super::*;
use crate::project::BusOutput;

impl App {
    /// A nested rack's output is owned by its group membership.
    fn bus_parent_output(&self, id: BusId) -> Option<BusOutput> {
        let rack = self.groups.iter().find(|group| group.bus_id == id.0 && group.is_rack())?;
        self.rack_parent_group(rack.id)
            .map(|parent| BusOutput::Bus(self.groups[parent].bus_id))
    }

    pub fn bus_output_options(&self, source: BusId) -> Vec<BusId> {
        if source == BusId::MIX || !self.buses.iter().any(|bus| bus.id == source)
            || self.bus_parent_output(source).is_some()
        {
            return Vec::new();
        }
        self.buses.iter().filter(|bus| {
            !self.groups.iter().any(|group| group.bus_id == bus.id.0)
                && self.bus_output_is_acyclic(source, bus.id)
        }).map(|bus| bus.id).collect()
    }

    fn bus_output_is_acyclic(&self, source: BusId, destination: BusId) -> bool {
        let mut cursor = destination;
        let mut seen = std::collections::HashSet::new();
        loop {
            if cursor == source || !seen.insert(cursor) {
                return false;
            }
            if cursor == BusId::MIX {
                return true;
            }
            let Some(bus) = self.buses.iter().find(|bus| bus.id == cursor) else {
                return false;
            };
            cursor = BusId(bus.output.destination().unwrap_or(BusId::MIX.0));
        }
    }

    pub fn set_bus_output_recorded(&mut self, source: BusId, destination: BusId) -> Result<(), String> {
        if !self.bus_output_options(source).contains(&destination) {
            return Err("Bus output destination is unavailable or would create a cycle".into());
        }
        let output = if destination == BusId::MIX { BusOutput::Mix } else { BusOutput::Bus(destination.0) };
        if self.buses.iter().find(|bus| bus.id == source).unwrap().output == output {
            return Ok(());
        }
        self.apply_recorded_scene_structure_mutation("Set bus output", |app| {
            app.buses.iter_mut().find(|bus| bus.id == source).unwrap().output = output;
            app.graph_controller().apply_bus_output_routing(source);
            app.push_solo_mutes();
            Ok(())
        })
    }

    /// Recall all destinations before validating, so scene switches never
    /// validate a mixture of the departing and arriving scene's edges.
    pub(super) fn normalize_bus_output_routing(&mut self) {
        for index in 0..self.buses.len() {
            let id = self.buses[index].id;
            if id == BusId::MIX {
                self.buses[index].output = BusOutput::Mix;
            } else if let Some(output) = self.bus_parent_output(id) {
                self.buses[index].output = output;
            }
        }
        for index in 0..self.buses.len() {
            let bus = &self.buses[index];
            if self.bus_parent_output(bus.id).is_some() { continue; }
            if let Some(destination) = bus.output.destination() {
                if !self.bus_output_is_acyclic(bus.id, BusId(destination)) {
                    self.buses[index].output = BusOutput::Mix;
                }
            }
        }
    }
}
