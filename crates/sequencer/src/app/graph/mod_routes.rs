use super::*;

impl GraphController<'_> {
    pub fn sync_current_pattern_mod_routes(&mut self) {
        let track_count = self.app.tracks.len();
        let connections = self.app.state.current_mod_connections();
        let mut desired: Vec<(i32, i32)> = Vec::with_capacity(connections.len());
        for connection in connections {
            // A route from an output or into an input the strip no longer
            // shows (the patch declares fewer/more outputs now) stays in the
            // scene but carries nothing until the port comes back.
            if connection.source_track >= track_count
                || connection.dest_input >= EXT_MOD_INPUT_COUNT
                || connection.source_output >= self.app.graph.track_mod_output_count(connection.source_track)
                || !self.mod_destination_input_shown(connection.destination, connection.dest_input)
            {
                continue;
            }
            if matches!(connection.destination, ModDestination::Track(dest) if dest == connection.source_track)
            {
                continue;
            }
            let source_id =
                self.app.graph.track_node_ids[connection.source_track].mod_out_ids[connection.source_output];
            let Some(dest_id) =
                self.resolve_mod_destination_input(connection.destination, connection.dest_input)
            else {
                continue;
            };
            if !desired.contains(&(source_id, dest_id)) {
                desired.push((source_id, dest_id));
            }
        }

        let applied = std::mem::take(&mut self.app.graph.applied_mod_routes);
        let mut changed = false;
        for (source_id, dest_id) in &applied {
            if !desired.contains(&(*source_id, *dest_id)) {
                changed = true;
                unsafe {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        *source_id,
                        0,
                        *dest_id,
                        0,
                    );
                }
            }
        }
        for (source_id, dest_id) in &desired {
            if !applied.contains(&(*source_id, *dest_id)) {
                changed = true;
                unsafe {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        *source_id,
                        0,
                        *dest_id,
                        0,
                    );
                }
            }
        }
        self.app.graph.applied_mod_routes = desired;
        if changed {
            self.app.state.publish_scheduler_snapshot();
        }
    }

    /// Point every track's mod input clips at its own `mod_trigger_lanes`
    /// lanes again after the tracks from `first_track` on shifted down
    /// (docs/mod-input-roles-spec.md §6). Creation sets the lane through the
    /// clip's initial state; only a delete reindexes tracks.
    pub(super) fn sync_mod_trigger_publish_targets(&mut self, first_track: usize) {
        for (track, nodes) in self.app.graph.track_node_ids.iter().enumerate().skip(first_track) {
            for (input, &clip_id) in nodes.mod_in_clip_ids.iter().enumerate() {
                if clip_id <= 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::instruments::track_modulator::MOD_IN_CLIP_STATE_PUBLISH as u64,
                            logical_id: clip_id as u64,
                            fvalue: crate::instruments::track_modulator::mod_in_clip_publish_value(
                                track, input,
                            ),
                        },
                    );
                }
            }
        }
    }

    pub(super) fn resolve_mod_destination_input(
        &self,
        destination: ModDestination,
        input: usize,
    ) -> Option<i32> {
        if input >= EXT_MOD_INPUT_COUNT {
            return None;
        }
        match destination {
            ModDestination::Track(track) => self
                .app
                .graph
                .track_node_ids
                .get(track)
                .map(|nodes| nodes.mod_in_clip_ids[input]),
            ModDestination::Bus(bus_id) => self
                .app
                .graph
                .bus_node_ids
                .iter()
                .find(|bus| bus.id == bus_id)
                .map(|nodes| nodes.mod_in_clip_ids[input]),
        }
    }

    /// Whether `destination` shows mod input `input`: a track gives up
    /// inputs to its extra outputs; a bus shows all four.
    fn mod_destination_input_shown(&self, destination: ModDestination, input: usize) -> bool {
        match destination {
            ModDestination::Track(track) => {
                input < self.app.graph.track_visible_mod_inputs(track)
            }
            ModDestination::Bus(_) => input < EXT_MOD_INPUT_COUNT,
        }
    }

    pub(super) fn validate_mod_destination(
        &self,
        source_track: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        if dest_input >= EXT_MOD_INPUT_COUNT {
            return Err("mod route input out of range".to_string());
        }
        match destination {
            ModDestination::Track(dest_track) => {
                if dest_track >= self.app.tracks.len() {
                    return Err("mod route destination track out of range".to_string());
                }
                if source_track == dest_track {
                    return Err("mod route cannot connect a track to itself".to_string());
                }
                if !self.mod_destination_input_shown(destination, dest_input) {
                    return Err(format!(
                        "track {} has no Ext{} input beside its mod outputs",
                        dest_track + 1,
                        dest_input + 1
                    ));
                }
            }
            ModDestination::Bus(bus_id) => {
                if !self
                    .app
                    .graph
                    .bus_node_ids
                    .iter()
                    .any(|bus| bus.id == bus_id)
                {
                    return Err("mod route destination bus not found".to_string());
                }
            }
        }
        Ok(())
    }

    pub fn set_mod_route_to_destination(
        &mut self,
        source_track: usize,
        source_output: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        if source_track >= track_count {
            return Err("mod route source track out of range".to_string());
        }
        if !self.app.graph.track_exposes_mod_output(source_track) {
            return Err("mod route source track has no mod output".to_string());
        }
        if source_output >= self.app.graph.track_mod_output_count(source_track) {
            return Err(format!(
                "mod route source track has no mod output {}",
                source_output + 1
            ));
        }
        self.validate_mod_destination(source_track, destination, dest_input)?;
        self.app.state.edit_current_mod_connections(|connections| {
            let connection = crate::sequencer::ModConnection {
                source_output,
                source_track,
                destination,
                dest_input,
            };
            if !connections.contains(&connection) {
                connections.push(connection);
            }
            Ok(())
        })?;
        self.sync_current_pattern_mod_routes();
        Ok(())
    }

    pub fn set_mod_route(
        &mut self,
        source_track: usize,
        dest_track: usize,
        dest_input: usize,
    ) -> Result<(), String> {
        self.set_mod_route_to_destination(
            source_track,
            0,
            ModDestination::Track(dest_track),
            dest_input,
        )
    }

    pub fn delete_mod_route_to_destination(
        &mut self,
        source_track: usize,
        source_output: usize,
        destination: ModDestination,
        dest_input: usize,
    ) -> Result<(), String> {
        self.app.state.edit_current_mod_connections(|connections| {
            connections.retain(|connection| {
                connection.source_track != source_track
                    || connection.source_output != source_output
                    || connection.destination != destination
                    || connection.dest_input != dest_input
            });
            Ok(())
        })?;
        self.sync_current_pattern_mod_routes();
        Ok(())
    }

    pub fn delete_mod_route(
        &mut self,
        source_track: usize,
        dest_track: usize,
        dest_input: usize,
    ) -> Result<(), String> {
        self.delete_mod_route_to_destination(
            source_track,
            0,
            ModDestination::Track(dest_track),
            dest_input,
        )
    }

}
