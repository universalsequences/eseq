use super::*;

pub(super) fn remove_track_lane_if_present<T>(lanes: &mut Vec<T>, track_idx: usize) {
    if track_idx < lanes.len() {
        lanes.remove(track_idx);
    }
}

pub(super) fn remap_optional_track_after_delete(track: usize, deleted_track: usize) -> Option<usize> {
    if track == deleted_track {
        None
    } else if track > deleted_track {
        Some(track - 1)
    } else {
        Some(track)
    }
}

pub(super) fn remap_graph_overrides_after_track_delete(
    overrides: &mut [ProjectGraphOverrides],
    deleted_track: usize,
) {
    for graph in overrides {
        for intrinsic in &mut graph.node_intrinsics {
            if let Some(route) = intrinsic.route.take() {
                intrinsic.route = match route {
                    crate::graph::ProjectGraphRouteOverride::None => {
                        Some(crate::graph::ProjectGraphRouteOverride::None)
                    }
                    crate::graph::ProjectGraphRouteOverride::Track(track) => {
                        remap_optional_track_after_delete(track, deleted_track)
                            .map(crate::graph::ProjectGraphRouteOverride::Track)
                    }
                };
            }
            if let Some(seed_from) = intrinsic.seed_from.take() {
                intrinsic.seed_from = Some(match seed_from {
                    crate::graph::ProjectGraphSeedFrom::Route => {
                        crate::graph::ProjectGraphSeedFrom::Route
                    }
                    crate::graph::ProjectGraphSeedFrom::Tracks(tracks) => {
                        crate::graph::ProjectGraphSeedFrom::Tracks(
                            tracks
                                .into_iter()
                                .filter_map(|track| {
                                    remap_optional_track_after_delete(track, deleted_track)
                                })
                                .collect(),
                        )
                    }
                });
            }
        }
    }
}

pub(super) fn remap_mod_connection_after_track_delete(
    connection: ModConnection,
    deleted_track: usize,
) -> Option<ModConnection> {
    if connection.source_track == deleted_track {
        return None;
    }
    let destination = match connection.destination {
        crate::sequencer::ModDestination::Track(track) if track == deleted_track => return None,
        crate::sequencer::ModDestination::Track(track) => {
            crate::sequencer::ModDestination::Track(if track > deleted_track {
                track - 1
            } else {
                track
            })
        }
        crate::sequencer::ModDestination::Bus(bus) => crate::sequencer::ModDestination::Bus(bus),
    };
    Some(ModConnection {
        source_track: if connection.source_track > deleted_track {
            connection.source_track - 1
        } else {
            connection.source_track
        },
        destination,
        dest_input: connection.dest_input,
    })
}

pub(super) fn mod_destination_valid_for_track_count(
    destination: crate::sequencer::ModDestination,
    track_count: usize,
) -> bool {
    match destination {
        crate::sequencer::ModDestination::Track(track) => track < track_count,
        crate::sequencer::ModDestination::Bus(_) => true,
    }
}

pub(super) fn sidechain_source_track(
    owner_track: usize,
    selection_idx: usize,
    total_tracks: usize,
) -> Option<usize> {
    if selection_idx == 0 {
        return None;
    }
    let mut current_idx = 0usize;
    for source_track in 0..total_tracks {
        if source_track == owner_track {
            continue;
        }
        current_idx += 1;
        if current_idx == selection_idx {
            return Some(source_track);
        }
    }
    None
}

pub(super) fn sidechain_selection_index(
    owner_track: usize,
    source_track: usize,
    total_tracks: usize,
) -> usize {
    if source_track >= total_tracks || source_track == owner_track {
        return 0;
    }
    let mut selection_idx = 0usize;
    for candidate in 0..total_tracks {
        if candidate == owner_track {
            continue;
        }
        selection_idx += 1;
        if candidate == source_track {
            return selection_idx;
        }
    }
    0
}

pub(super) fn remap_sidechain_selection_after_track_delete(
    owner_track_old: usize,
    selection_idx: usize,
    deleted_track: usize,
    old_track_count: usize,
) -> usize {
    let Some(source_old) = sidechain_source_track(owner_track_old, selection_idx, old_track_count)
    else {
        return 0;
    };
    if source_old == deleted_track {
        return 0;
    }
    let owner_new = if owner_track_old > deleted_track {
        owner_track_old - 1
    } else {
        owner_track_old
    };
    let source_new = if source_old > deleted_track {
        source_old - 1
    } else {
        source_old
    };
    sidechain_selection_index(owner_new, source_new, old_track_count - 1)
}

pub(super) fn remap_snapshot_sidechain_references_after_track_delete(
    snapshot: &mut PatternSnapshot,
    effect_descriptors: &[Vec<EffectDescriptor>],
    deleted_track: usize,
    old_track_count: usize,
) {
    for owner_track in 0..old_track_count {
        if owner_track == deleted_track || owner_track >= snapshot.effect_slots.len() {
            continue;
        }
        let Some(track_descs) = effect_descriptors.get(owner_track) else {
            continue;
        };
        for (slot_idx, slot) in snapshot.effect_slots[owner_track].iter_mut().enumerate() {
            let Some(desc) = track_descs.get(slot_idx) else {
                continue;
            };
            let num_params = slot.num_params as usize;
            for param_idx in 0..num_params.min(desc.params.len()) {
                if !matches!(
                    desc.params[param_idx].host_control,
                    Some(HostControl::FxSidechain { .. })
                ) {
                    continue;
                }
                let remapped = remap_sidechain_selection_after_track_delete(
                    owner_track,
                    slot.defaults
                        .get(param_idx)
                        .copied()
                        .unwrap_or(0.0)
                        .round()
                        .max(0.0) as usize,
                    deleted_track,
                    old_track_count,
                ) as f32;
                if param_idx < slot.defaults.len() {
                    slot.defaults[param_idx] = remapped;
                }
                for step in 0..MAX_STEPS {
                    let selection = slot.plocks.get(step)
                        .and_then(|params| params.get(param_idx))
                        .and_then(|value| *value);
                    if let (Some(selection), Some(value)) = (
                        selection,
                        slot.plocks.get_mut(step).and_then(|params| params.get_mut(param_idx)),
                    ) {
                        *value = Some(remap_sidechain_selection_after_track_delete(
                            owner_track,
                            selection.round().max(0.0) as usize,
                            deleted_track,
                            old_track_count,
                        ) as f32);
                    }
                }
            }
        }
    }
}
