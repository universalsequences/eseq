use super::*;

impl GraphController<'_> {
    pub fn ensure_bus_graph_node(&mut self, id: BusId, name: &str) {
        if id == BusId::MIX || self.app.graph.bus_node_ids.iter().any(|bus| bus.id == id) {
            return;
        }

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let safe_name: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let left_name = format!("{safe_name}_L");
        let right_name = format!("{safe_name}_R");
        let merge_name = CString::new(format!("{safe_name}_merge")).unwrap();
        let gate_name = CString::new(format!("{safe_name}_gate")).unwrap();
        let volume_name = CString::new(format!("{safe_name}_volume")).unwrap();
        let mod_in_clip_ids = std::array::from_fn(|input| {
            let mod_in_name =
                CString::new(format!("{safe_name}_mod_in{}_clip", input + 1)).unwrap();
            unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::instruments::track_modulator::mod_in_clip_vtable(),
                    crate::instruments::track_modulator::MOD_IN_CLIP_STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_in_name.as_ptr(),
                    1,
                    1,
                    std::ptr::null(),
                    0,
                )
            }
        });
        let left_id = match add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &left_name,
            "ensure_bus_graph_node left bus input",
        ) {
            Ok(node_id) => node_id,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        let right_id = match add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &right_name,
            "ensure_bus_graph_node right bus input",
        ) {
            Ok(node_id) => node_id,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        let merge_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                merge_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let volume_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                volume_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let gate_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                gate_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, left_id, 0, merge_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, right_id, 0, merge_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, merge_id, 0, gate_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, merge_id, 1, gate_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, gate_id, 0, volume_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, gate_id, 1, volume_id, 1);
        }
        let pdc_id = super::latency::add_pdc_node(self.app.graph.lg.0, &format!("{safe_name}_pdc"));
        let meter_id = crate::effects::peak_meter::add_peak_meter_node(
            self.app.graph.lg.0,
            &format!("{safe_name}_meter"),
        );
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, meter_id);
            crate::audiograph::graph_connect(self.app.graph.lg.0, volume_id, 0, pdc_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, volume_id, 1, pdc_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pdc_id, 0, meter_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pdc_id, 1, meter_id, 1);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                pdc_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                pdc_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
        }
        self.app.graph.bus_node_ids.push(super::super::BusNodeIds {
            id,
            left_id,
            right_id,
            merge_id,
            gate_id,
            volume_id,
            pdc_id,
            meter_id,
            mod_in_clip_ids,
        });
        self.app.publish_bus_gate_runtime();
    }

    /// Makes the graph bus registry exactly mirror the current project bus
    /// registry, including ordering. Several realtime/UI bridges intentionally
    /// use compact bus indices, so this invariant must be restored whenever a
    /// project replaces `App::buses` while retaining the live graph.
    pub fn reconcile_bus_graph_nodes(&mut self) -> Result<(), String> {
        let project_buses = self
            .app
            .buses
            .iter()
            .map(|bus| (bus.id, bus.name.clone()))
            .collect::<Vec<_>>();
        let stale_ids = self
            .app
            .graph
            .bus_node_ids
            .iter()
            .map(|nodes| nodes.id)
            .filter(|id| !project_buses.iter().any(|(project_id, _)| project_id == id))
            .collect::<Vec<_>>();
        for id in stale_ids {
            self.delete_bus_graph_node(id);
        }
        for (id, name) in &project_buses {
            self.ensure_bus_graph_node(*id, name);
        }

        for (id, name) in &project_buses {
            if !self
                .app
                .graph
                .bus_node_ids
                .iter()
                .any(|nodes| nodes.id == *id)
            {
                return Err(format!(
                    "Graph nodes for bus '{name}' ({}) were not created",
                    id.0
                ));
            }
        }
        self.app.graph.bus_node_ids.sort_by_key(|nodes| {
            project_buses
                .iter()
                .position(|(id, _)| *id == nodes.id)
                .expect("graph bus membership was validated before sorting")
        });
        self.app.publish_bus_gate_runtime();
        self.app.refresh_latency_compensation();
        Ok(())
    }

    pub fn delete_bus_graph_node(&mut self, id: BusId) {
        let Some(pos) = self
            .app
            .graph
            .bus_node_ids
            .iter()
            .position(|bus| bus.id == id)
        else {
            return;
        };
        let bus = self.app.graph.bus_node_ids.remove(pos);
        unsafe {
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, bus.meter_id);
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.pdc_id,
                0,
                bus.meter_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.pdc_id,
                1,
                bus.meter_id,
                1,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.pdc_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.pdc_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            crate::audiograph::graph_disconnect(self.app.graph.lg.0, bus.volume_id, 0, bus.pdc_id, 0);
            crate::audiograph::graph_disconnect(self.app.graph.lg.0, bus.volume_id, 1, bus.pdc_id, 1);
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.left_id,
                0,
                bus.merge_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.right_id,
                0,
                bus.merge_id,
                1,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.merge_id,
                0,
                bus.gate_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.merge_id,
                1,
                bus.gate_id,
                1,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.gate_id,
                0,
                bus.volume_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                bus.gate_id,
                1,
                bus.volume_id,
                1,
            );
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.merge_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.gate_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.volume_id);
            self.app.invalidate_latency_pad_cache();
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.pdc_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.meter_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.left_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, bus.right_id);
            for &mod_in_clip_id in &bus.mod_in_clip_ids {
                crate::audiograph::delete_node(self.app.graph.lg.0, mod_in_clip_id);
            }
        }
        self.app.publish_bus_gate_runtime();
    }

    pub(super) fn disconnect_delay_output_from_all(&self, delay_id: i32) {
        unsafe {
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                delay_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_disconnect(
                self.app.graph.lg.0,
                delay_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            for bus in &self.app.graph.bus_node_ids {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    delay_id,
                    0,
                    bus.left_id,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    delay_id,
                    1,
                    bus.right_id,
                    0,
                );
            }
        }
    }

    pub(super) fn connect_delay_output_to(&self, delay_id: i32, output: &TrackOutput) {
        unsafe {
            match output {
                TrackOutput::Mix => {
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        delay_id,
                        0,
                        self.app.graph.bus_l_id,
                        0,
                    );
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        delay_id,
                        1,
                        self.app.graph.bus_r_id,
                        0,
                    );
                }
                TrackOutput::Bus(id) => {
                    if let Some(bus) = self.app.graph.bus_node_ids.iter().find(|bus| bus.id == *id)
                    {
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            0,
                            bus.left_id,
                            0,
                        );
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            1,
                            bus.right_id,
                            0,
                        );
                    } else {
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            0,
                            self.app.graph.bus_l_id,
                            0,
                        );
                        crate::audiograph::graph_connect(
                            self.app.graph.lg.0,
                            delay_id,
                            1,
                            self.app.graph.bus_r_id,
                            0,
                        );
                    }
                }
                TrackOutput::None => {}
            }
        }
    }

    pub fn apply_track_output_routing(&mut self, track_idx: usize) {
        let Some(nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return;
        };
        let output = self.app.state.pattern.track_params[track_idx].output();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.disconnect_delay_output_from_all(nodes.pdc_id);
        self.connect_delay_output_to(nodes.pdc_id, &output);
        self.app.refresh_latency_compensation();
    }

    pub fn apply_track_bus_sends(&mut self, track_idx: usize) {
        let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) else {
            return;
        };
        let delay_id = nodes.delay_id;
        let mut old_sends = std::mem::take(&mut nodes.bus_send_ids);
        let requested_sends = self.app.state.pattern.track_params[track_idx].sends();
        let bus_nodes = self.app.graph.bus_node_ids.clone();
        let lg = self.app.graph.lg.0;

        let _batch = GraphEditBatchGuard::new(lg);
        let mut next_send_nodes = Vec::new();

        for send in requested_sends {
            if send.amount <= 0.0 {
                continue;
            }
            let Some(bus) = bus_nodes.iter().find(|bus| bus.id == send.destination) else {
                continue;
            };

            if let Some(pos) = old_sends
                .iter()
                .position(|nodes| nodes.destination == send.destination)
            {
                let existing = old_sends.remove(pos);
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        lg,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: existing.left_id as u64,
                            fvalue: send.amount,
                        },
                    );
                    crate::audiograph::params_push_wrapper(
                        lg,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: existing.right_id as u64,
                            fvalue: send.amount,
                        },
                    );
                }
                next_send_nodes.push(existing);
                continue;
            }

            let left_name = format!("track_{track_idx}_send_{}_L", send.destination.0);
            let right_name = format!("track_{track_idx}_send_{}_R", send.destination.0);
            let left_id = match add_gain_node_checked(
                lg,
                send.amount,
                &left_name,
                "apply_track_bus_sends left send",
            ) {
                Ok(node_id) => node_id,
                Err(error) => {
                    eprintln!("{error}");
                    continue;
                }
            };
            let right_id = match add_gain_node_checked(
                lg,
                send.amount,
                &right_name,
                "apply_track_bus_sends right send",
            ) {
                Ok(node_id) => node_id,
                Err(error) => {
                    eprintln!("{error}");
                    unsafe {
                        crate::audiograph::delete_node(lg, left_id);
                    }
                    continue;
                }
            };
            let pdc_id = super::latency::add_pdc_node(
                lg,
                &format!("track_{track_idx}_send_{}_pdc", send.destination.0),
            );
            unsafe {
                crate::audiograph::graph_connect(lg, delay_id, 0, pdc_id, 0);
                crate::audiograph::graph_connect(lg, delay_id, 1, pdc_id, 1);
                crate::audiograph::graph_connect(lg, pdc_id, 0, left_id, 0);
                crate::audiograph::graph_connect(lg, pdc_id, 1, right_id, 0);
                crate::audiograph::graph_connect(lg, left_id, 0, bus.left_id, 0);
                crate::audiograph::graph_connect(lg, right_id, 0, bus.right_id, 0);
            }
            next_send_nodes.push(super::super::BusSendNodeIds {
                destination: send.destination,
                left_id,
                right_id,
                pdc_id,
            });
        }

        if !old_sends.is_empty() {
            // Send PDC nodes die below and their ids may be reused.
            self.app.invalidate_latency_pad_cache();
        }
        for send in old_sends {
            if let Some(bus) = bus_nodes.iter().find(|bus| bus.id == send.destination) {
                unsafe {
                    crate::audiograph::graph_disconnect(lg, delay_id, 0, send.pdc_id, 0);
                    crate::audiograph::graph_disconnect(lg, delay_id, 1, send.pdc_id, 1);
                    crate::audiograph::graph_disconnect(lg, send.pdc_id, 0, send.left_id, 0);
                    crate::audiograph::graph_disconnect(lg, send.pdc_id, 1, send.right_id, 0);
                    crate::audiograph::graph_disconnect(lg, send.left_id, 0, bus.left_id, 0);
                    crate::audiograph::graph_disconnect(lg, send.right_id, 0, bus.right_id, 0);
                }
            }
            unsafe {
                crate::audiograph::delete_node(lg, send.left_id);
                crate::audiograph::delete_node(lg, send.right_id);
                crate::audiograph::delete_node(lg, send.pdc_id);
            }
        }

        let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) else {
            return;
        };
        nodes.bus_send_ids = next_send_nodes;
        self.app.refresh_latency_compensation();
    }

}
