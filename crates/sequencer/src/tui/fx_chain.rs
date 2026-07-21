use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::lisp_host::{self, DylibLease};
use crate::sequencer::BusId;

use super::App;

pub(super) struct FxGraphEditBatch {
    lg: *mut crate::audiograph::LiveGraph,
    pub serial: u64,
}

impl FxGraphEditBatch {
    pub fn new(lg: *mut crate::audiograph::LiveGraph) -> Self {
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        let serial = unsafe { crate::audiograph::graph_edit_current_batch_serial(lg) };
        debug_assert!(serial > 0);
        Self { lg, serial }
    }
}

impl Drop for FxGraphEditBatch {
    fn drop(&mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum FxChainLocator {
    Track(usize),
    Bus(BusId),
    RackSlot { track: usize, slot: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FxLeaseSlotRemoval {
    Clear,
    Shift,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RetainedEffectSource {
    NativeBuiltin { name: String },
    Compiled {
        name: String,
        source: String,
        asset_base: Option<std::path::PathBuf>,
        origin: crate::lisp_host::DGenSourceOrigin,
    },
}

struct RetiredFxLease {
    applied_after: u64,
    _lease: DylibLease,
}

fn insert_empty_slot<T>(row: &mut [Option<T>], slot_idx: usize) {
    for idx in (slot_idx + 1..row.len()).rev() {
        row[idx] = row[idx - 1].take();
    }
    row[slot_idx] = None;
}

fn remove_slot<T>(row: &mut [Option<T>], slot_idx: usize) -> Option<T> {
    let removed = row[slot_idx].take();
    for idx in slot_idx..row.len().saturating_sub(1) {
        row[idx] = row[idx + 1].take();
    }
    if let Some(last) = row.last_mut() {
        *last = None;
    }
    removed
}

fn move_slot<T>(row: &mut [Option<T>], source_slot: usize, target_slot: usize) {
    if source_slot == target_slot {
        return;
    }
    let value = row[source_slot].take();
    if source_slot < target_slot {
        for idx in source_slot..target_slot {
            row[idx] = row[idx + 1].take();
        }
    } else {
        for idx in (target_slot + 1..=source_slot).rev() {
            row[idx] = row[idx - 1].take();
        }
    }
    row[target_slot] = value;
}

#[derive(Default)]
pub(super) struct FxChainLeaseStore {
    rows: HashMap<FxChainLocator, Vec<Option<DylibLease>>>,
    sources: HashMap<FxChainLocator, Vec<Option<RetainedEffectSource>>>,
    retired: Vec<RetiredFxLease>,
}

impl FxChainLeaseStore {
    fn row_slot(locator: FxChainLocator, slot_idx: usize) -> Result<usize, String> {
        match locator {
            FxChainLocator::Track(_) => slot_idx
                .checked_sub(crate::effects::BUILTIN_SLOT_COUNT)
                .ok_or_else(|| "Built-in track FX slots do not own dynamic-library leases".into()),
            FxChainLocator::Bus(_) | FxChainLocator::RackSlot { .. } => Ok(slot_idx),
        }
    }

    fn row_mut(&mut self, locator: FxChainLocator) -> &mut Vec<Option<DylibLease>> {
        self.rows.entry(locator).or_insert_with(|| {
            std::iter::repeat_with(|| None)
                .take(lisp_host::MAX_CUSTOM_FX)
                .collect()
        })
    }

    fn source_row_mut(
        &mut self,
        locator: FxChainLocator,
    ) -> &mut Vec<Option<RetainedEffectSource>> {
        self.sources.entry(locator).or_insert_with(|| {
            std::iter::repeat_with(|| None)
                .take(lisp_host::MAX_CUSTOM_FX)
                .collect()
        })
    }

    pub fn source(
        &self,
        locator: FxChainLocator,
        slot_idx: usize,
    ) -> Option<&RetainedEffectSource> {
        let slot_idx = Self::row_slot(locator, slot_idx).ok()?;
        self.sources.get(&locator)?.get(slot_idx)?.as_ref()
    }

    pub fn set_source(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        source: Option<RetainedEffectSource>,
    ) -> Result<(), String> {
        let slot_idx = Self::row_slot(locator, slot_idx)?;
        let row = self.source_row_mut(locator);
        let target = row
            .get_mut(slot_idx)
            .ok_or_else(|| format!("FX source slot {} is out of range", slot_idx + 1))?;
        *target = source;
        Ok(())
    }

    pub fn reclaim_applied(&mut self, applied_batch_serial: u64) {
        self.retired
            .retain(|lease| lease.applied_after > applied_batch_serial);
    }

    pub fn set(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        lease: Option<DylibLease>,
        retire_after: u64,
    ) -> Result<(), String> {
        let slot_idx = Self::row_slot(locator, slot_idx)?;
        let row = self.row_mut(locator);
        let slot = row.get_mut(slot_idx).ok_or_else(|| {
            format!(
                "FX lease slot {} is outside the {}-slot chain",
                slot_idx + 1,
                lisp_host::MAX_CUSTOM_FX
            )
        })?;
        if slot.is_some() && retire_after == 0 {
            return Err("Replacing an FX lease requires an edit-batch retirement serial".into());
        }
        let retired = std::mem::replace(slot, lease);
        if let Some(retired) = retired {
            self.retired.push(RetiredFxLease {
                applied_after: retire_after,
                _lease: retired,
            });
        }
        Ok(())
    }

    pub fn insert_empty_slot(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
    ) -> Result<(), String> {
        let slot_idx = Self::row_slot(locator, slot_idx)?;
        let row = self.row_mut(locator);
        if slot_idx >= row.len() {
            return Err(format!("FX lease slot {} is out of range", slot_idx + 1));
        }
        insert_empty_slot(row, slot_idx);
        insert_empty_slot(self.source_row_mut(locator), slot_idx);
        Ok(())
    }

    pub fn remove_slot(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        retire_after: u64,
    ) -> Result<(), String> {
        let slot_idx = Self::row_slot(locator, slot_idx)?;
        let row = self.row_mut(locator);
        if slot_idx >= row.len() {
            return Err(format!("FX lease slot {} is out of range", slot_idx + 1));
        }
        if row[slot_idx].is_some() && retire_after == 0 {
            return Err("Removing an FX lease requires an edit-batch retirement serial".into());
        }
        let retired = remove_slot(row, slot_idx);
        remove_slot(self.source_row_mut(locator), slot_idx);
        if let Some(retired) = retired {
            self.retired.push(RetiredFxLease {
                applied_after: retire_after,
                _lease: retired,
            });
        }
        Ok(())
    }

    pub fn move_slot(
        &mut self,
        locator: FxChainLocator,
        source_slot: usize,
        target_slot: usize,
    ) -> Result<(), String> {
        let source_slot = Self::row_slot(locator, source_slot)?;
        let target_slot = Self::row_slot(locator, target_slot)?;
        let row = self.row_mut(locator);
        if source_slot >= row.len() || target_slot >= row.len() {
            return Err("FX lease move is out of range".to_string());
        }
        move_slot(row, source_slot, target_slot);
        move_slot(self.source_row_mut(locator), source_slot, target_slot);
        Ok(())
    }

    pub fn remap_slots(
        &mut self,
        locator: FxChainLocator,
        desired_source_slots: &[usize],
        retire_after: u64,
    ) -> Result<(), String> {
        let source_offsets = desired_source_slots
            .iter()
            .map(|slot| Self::row_slot(locator, *slot))
            .collect::<Result<Vec<_>, _>>()?;
        if source_offsets.len() > lisp_host::MAX_CUSTOM_FX
            || source_offsets.iter().any(|slot| *slot >= lisp_host::MAX_CUSTOM_FX)
            || source_offsets.iter().enumerate().any(|(index, slot)| {
                source_offsets[..index].contains(slot)
            })
        {
            return Err("FX lease remap is out of range".to_string());
        }
        let mut old = self.rows.remove(&locator).unwrap_or_else(|| {
            std::iter::repeat_with(|| None)
                .take(lisp_host::MAX_CUSTOM_FX)
                .collect()
        });
        let mut old_sources = self.sources.remove(&locator).unwrap_or_else(|| {
            std::iter::repeat_with(|| None)
                .take(lisp_host::MAX_CUSTOM_FX)
                .collect()
        });
        if retire_after == 0
            && old.iter().enumerate().any(|(slot, lease)| {
                lease.is_some() && !source_offsets.contains(&slot)
            })
        {
            self.rows.insert(locator, old);
            self.sources.insert(locator, old_sources);
            return Err("Retiring FX leases requires an edit-batch serial".to_string());
        }
        let mut remapped = std::iter::repeat_with(|| None)
            .take(lisp_host::MAX_CUSTOM_FX)
            .collect::<Vec<_>>();
        let mut remapped_sources = std::iter::repeat_with(|| None)
            .take(lisp_host::MAX_CUSTOM_FX)
            .collect::<Vec<_>>();
        for (target, source) in source_offsets.into_iter().enumerate() {
            remapped[target] = old[source].take();
            remapped_sources[target] = old_sources[source].take();
        }
        self.retired
            .extend(old.into_iter().flatten().map(|lease| RetiredFxLease {
                applied_after: retire_after,
                _lease: lease,
            }));
        self.rows.insert(locator, remapped);
        self.sources.insert(locator, remapped_sources);
        Ok(())
    }

    pub fn retire_host(
        &mut self,
        locator: FxChainLocator,
        retire_after: u64,
    ) -> Result<(), String> {
        let Some(row) = self.rows.remove(&locator) else {
            self.sources.remove(&locator);
            return Ok(());
        };
        let sources = self.sources.remove(&locator);
        if row.iter().any(Option::is_some) && retire_after == 0 {
            self.rows.insert(locator, row);
            if let Some(sources) = sources {
                self.sources.insert(locator, sources);
            }
            return Err("Retiring an FX host requires an edit-batch retirement serial".into());
        }
        self.retired
            .extend(row.into_iter().flatten().map(|lease| RetiredFxLease {
                applied_after: retire_after,
                _lease: lease,
            }));
        Ok(())
    }

    pub fn retire_buses(&mut self, retire_after: u64) -> Result<(), String> {
        let buses = self
            .rows
            .keys()
            .copied()
            .filter(|locator| matches!(locator, FxChainLocator::Bus(_)))
            .collect::<Vec<_>>();
        for locator in buses {
            self.retire_host(locator, retire_after)?;
        }
        Ok(())
    }

    pub fn retire_tracks(&mut self, retire_after: u64) -> Result<(), String> {
        let tracks = self
            .rows
            .keys()
            .copied()
            .filter(|locator| {
                matches!(
                    locator,
                    FxChainLocator::Track(_) | FxChainLocator::RackSlot { .. }
                )
            })
            .collect::<Vec<_>>();
        for locator in tracks {
            self.retire_host(locator, retire_after)?;
        }
        Ok(())
    }

    pub fn reindex_tracks_after_delete(&mut self, deleted_track: usize) {
        let rows = std::mem::take(&mut self.rows);
        self.rows = rows
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::Track(track) if track > deleted_track => {
                        FxChainLocator::Track(track - 1)
                    }
                    FxChainLocator::RackSlot { track, slot } if track > deleted_track => {
                        FxChainLocator::RackSlot {
                            track: track - 1,
                            slot,
                        }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
        let sources = std::mem::take(&mut self.sources);
        self.sources = sources
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::Track(track) if track > deleted_track => {
                        FxChainLocator::Track(track - 1)
                    }
                    FxChainLocator::RackSlot { track, slot } if track > deleted_track => {
                        FxChainLocator::RackSlot {
                            track: track - 1,
                            slot,
                        }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
    }

    pub fn reindex_tracks_move_last_to(&mut self, last: usize, target: usize) {
        let remap = |track: usize| {
            if track == last {
                target
            } else if (target..last).contains(&track) {
                track + 1
            } else {
                track
            }
        };
        let rows = std::mem::take(&mut self.rows);
        self.rows = rows.into_iter().map(|(locator, row)| {
            let locator = match locator {
                FxChainLocator::Track(track) => FxChainLocator::Track(remap(track)),
                FxChainLocator::RackSlot { track, slot } => FxChainLocator::RackSlot {
                    track: remap(track),
                    slot,
                },
                other => other,
            };
            (locator, row)
        }).collect();
        let sources = std::mem::take(&mut self.sources);
        self.sources = sources.into_iter().map(|(locator, row)| {
            let locator = match locator {
                FxChainLocator::Track(track) => FxChainLocator::Track(remap(track)),
                FxChainLocator::RackSlot { track, slot } => FxChainLocator::RackSlot {
                    track: remap(track),
                    slot,
                },
                other => other,
            };
            (locator, row)
        }).collect();
    }

    pub fn reindex_rack_slots_after_delete(&mut self, track: usize, deleted_slot: usize) {
        let rows = std::mem::take(&mut self.rows);
        self.rows = rows
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::RackSlot { track: owner, slot }
                        if owner == track && slot > deleted_slot =>
                    {
                        FxChainLocator::RackSlot {
                            track: owner,
                            slot: slot - 1,
                        }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
        let sources = std::mem::take(&mut self.sources);
        self.sources = sources
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::RackSlot { track: owner, slot }
                        if owner == track && slot > deleted_slot =>
                    {
                        FxChainLocator::RackSlot {
                            track: owner,
                            slot: slot - 1,
                        }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
    }

    pub fn move_rack_slot_host(&mut self, track: usize, source_slot: usize, target_slot: usize) {
        if source_slot == target_slot {
            return;
        }
        let rows = std::mem::take(&mut self.rows);
        self.rows = rows
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::RackSlot { track: owner, slot } if owner == track => {
                        let slot = if slot == source_slot {
                            target_slot
                        } else if source_slot < target_slot
                            && (source_slot + 1..=target_slot).contains(&slot)
                        {
                            slot - 1
                        } else if target_slot < source_slot
                            && (target_slot..source_slot).contains(&slot)
                        {
                            slot + 1
                        } else {
                            slot
                        };
                        FxChainLocator::RackSlot { track: owner, slot }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
        let sources = std::mem::take(&mut self.sources);
        self.sources = sources
            .into_iter()
            .map(|(locator, row)| {
                let locator = match locator {
                    FxChainLocator::RackSlot { track: owner, slot } if owner == track => {
                        let slot = if slot == source_slot {
                            target_slot
                        } else if source_slot < target_slot
                            && (source_slot + 1..=target_slot).contains(&slot)
                        {
                            slot - 1
                        } else if target_slot < source_slot
                            && (target_slot..source_slot).contains(&slot)
                        {
                            slot + 1
                        } else {
                            slot
                        };
                        FxChainLocator::RackSlot { track: owner, slot }
                    }
                    other => other,
                };
                (locator, row)
            })
            .collect();
    }

    pub fn move_host(
        &mut self,
        source: FxChainLocator,
        target: FxChainLocator,
    ) -> Result<(), String> {
        if source == target {
            return Ok(());
        }
        if self.rows.contains_key(&target) {
            return Err(format!("FX lease target host {target:?} already exists"));
        }
        if let Some(row) = self.rows.remove(&source) {
            self.rows.insert(target, row);
        }
        if let Some(row) = self.sources.remove(&source) {
            self.sources.insert(target, row);
        }
        Ok(())
    }

    pub(super) fn contains_host(&self, locator: FxChainLocator) -> bool {
        self.rows.contains_key(&locator)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StereoEndpoint {
    pub node_id: i32,
    pub channels: usize,
}

impl StereoEndpoint {
    fn normalized(self) -> Self {
        Self {
            node_id: self.node_id,
            channels: self.channels.clamp(1, 2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainSuccessor {
    StereoNode(StereoEndpoint),
    MonoPair { left: i32, right: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FxChainSlotView {
    pub node_id: i32,
    pub modulator_node_id: i32,
    pub input_channels: usize,
    pub output_channels: usize,
}

impl FxChainSlotView {
    fn input_endpoint(self) -> StereoEndpoint {
        StereoEndpoint {
            node_id: self.node_id,
            channels: self.input_channels,
        }
        .normalized()
    }

    fn output_endpoint(self) -> StereoEndpoint {
        StereoEndpoint {
            node_id: self.node_id,
            channels: self.output_channels,
        }
        .normalized()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FxChainHost {
    pub locator: FxChainLocator,
    pub label: String,
    pub predecessor: StereoEndpoint,
    pub successor: ChainSuccessor,
    pub slots: Vec<FxChainSlotView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FxSlotWiring {
    pub predecessor: StereoEndpoint,
    pub successor: ChainSuccessor,
    pub existing_node_id: Option<i32>,
}

impl FxSlotWiring {
    pub fn stereo_successor(self) -> Result<StereoEndpoint, String> {
        match self.successor {
            ChainSuccessor::StereoNode(endpoint) => Ok(endpoint),
            ChainSuccessor::MonoPair { .. } => {
                Err("FX operation requires a stereo-node successor".to_string())
            }
        }
    }
}

impl FxChainHost {
    pub fn wiring_for(&self, slot_idx: usize) -> Result<FxSlotWiring, String> {
        if slot_idx >= self.slots.len() {
            return Err(format!(
                "{} effect slot {} is out of range",
                self.label,
                slot_idx + 1
            ));
        }

        let predecessor = self.slots[..slot_idx]
            .iter()
            .rev()
            .copied()
            .find(|slot| slot.node_id > 0)
            .map(FxChainSlotView::output_endpoint)
            .unwrap_or(self.predecessor)
            .normalized();

        let successor = self.slots[slot_idx + 1..]
            .iter()
            .copied()
            .find(|slot| slot.node_id > 0)
            .map(|slot| ChainSuccessor::StereoNode(slot.input_endpoint()))
            .unwrap_or(self.successor);

        let existing_node_id =
            (self.slots[slot_idx].node_id > 0).then_some(self.slots[slot_idx].node_id);
        Ok(FxSlotWiring {
            predecessor,
            successor,
            existing_node_id,
        })
    }

    fn connections(&self) -> Vec<(StereoEndpoint, ChainSuccessor)> {
        let mut connections = Vec::new();
        let mut predecessor = self.predecessor.normalized();
        for slot in self.slots.iter().copied().filter(|slot| slot.node_id > 0) {
            let input = slot.input_endpoint();
            connections.push((predecessor, ChainSuccessor::StereoNode(input)));
            predecessor = slot.output_endpoint();
        }
        connections.push((predecessor, self.successor));
        connections
    }
}

impl App {
    fn fx_ext_mod_input_nodes(
        &self,
        locator: FxChainLocator,
    ) -> Option<[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]> {
        match locator {
            FxChainLocator::Track(track) => self
                .graph
                .track_node_ids
                .get(track)
                .map(|nodes| nodes.mod_in_clip_ids),
            FxChainLocator::Bus(bus_id) => self
                .graph
                .bus_node_ids
                .iter()
                .find(|nodes| nodes.id == bus_id)
                .map(|nodes| nodes.mod_in_clip_ids),
            FxChainLocator::RackSlot { track, .. } => self
                .graph
                .track_node_ids
                .get(track)
                .map(|nodes| nodes.mod_in_clip_ids),
        }
    }

    pub(super) fn fx_chain_host(&self, locator: FxChainLocator) -> Result<FxChainHost, String> {
        match locator {
            FxChainLocator::Track(track_idx) => {
                let nodes = self
                    .graph
                    .track_node_ids
                    .get(track_idx)
                    .ok_or_else(|| format!("Track {} graph nodes not found", track_idx + 1))?;
                let slots = self
                    .state
                    .pattern
                    .effect_chains
                    .get(track_idx)
                    .ok_or_else(|| format!("Track {} effect chain not found", track_idx + 1))?;
                let descriptors =
                    self.graph
                        .effect_descriptors
                        .get(track_idx)
                        .ok_or_else(|| {
                            format!("Track {} effect descriptors not found", track_idx + 1)
                        })?;
                if slots.len() != descriptors.len() {
                    return Err(format!(
                        "Track {} FX state is misaligned: {} slots, {} descriptors",
                        track_idx + 1,
                        slots.len(),
                        descriptors.len()
                    ));
                }
                Ok(FxChainHost {
                    locator,
                    label: self
                        .tracks
                        .get(track_idx)
                        .cloned()
                        .unwrap_or_else(|| format!("Track {}", track_idx + 1)),
                    predecessor: StereoEndpoint {
                        node_id: nodes.pan_id,
                        channels: 2,
                    },
                    successor: ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: nodes.delay_id,
                        channels: 2,
                    }),
                    slots: slots
                        .iter()
                        .zip(descriptors)
                        .map(|(slot, descriptor)| FxChainSlotView {
                            node_id: slot.node_id.load(Ordering::Relaxed) as i32,
                            modulator_node_id: slot.modulator_node_id.load(Ordering::Relaxed)
                                as i32,
                            input_channels: descriptor.input_channels,
                            output_channels: descriptor.output_channels,
                        })
                        .collect(),
                })
            }
            FxChainLocator::Bus(bus_id) => {
                let bus = self
                    .buses
                    .iter()
                    .find(|bus| bus.id == bus_id)
                    .ok_or_else(|| format!("Bus {} not found", bus_id.0))?;
                let nodes = self
                    .graph
                    .bus_node_ids
                    .iter()
                    .find(|nodes| nodes.id == bus_id)
                    .ok_or_else(|| format!("Graph nodes for bus '{}' not found", bus.name))?;
                if bus.effect_slots.len() != bus.effect_descriptors.len() {
                    return Err(format!(
                        "Bus '{}' FX state is misaligned: {} slots, {} descriptors",
                        bus.name,
                        bus.effect_slots.len(),
                        bus.effect_descriptors.len()
                    ));
                }
                Ok(FxChainHost {
                    locator,
                    label: bus.name.clone(),
                    predecessor: StereoEndpoint {
                        node_id: nodes.gate_id,
                        channels: 2,
                    },
                    successor: ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: nodes.volume_id,
                        channels: 2,
                    }),
                    slots: bus
                        .effect_slots
                        .iter()
                        .zip(&bus.effect_descriptors)
                        .map(|(slot, descriptor)| FxChainSlotView {
                            node_id: slot.node_id as i32,
                            modulator_node_id: slot.modulator_node_id as i32,
                            input_channels: descriptor.input_channels,
                            output_channels: descriptor.output_channels,
                        })
                        .collect(),
                })
            }
            FxChainLocator::RackSlot { track, slot } => {
                let track_nodes = self
                    .graph
                    .track_node_ids
                    .get(track)
                    .ok_or_else(|| format!("Track {} graph nodes not found", track + 1))?;
                let slot_nodes = track_nodes.rack_slots.get(slot).ok_or_else(|| {
                    format!(
                        "Track {} rack slot {} graph nodes not found",
                        track + 1,
                        slot + 1
                    )
                })?;
                let rack_tracks = self.state.pattern.rack_tracks.lock().unwrap();
                let rack_slot = rack_tracks
                    .get(track)
                    .and_then(Option::as_ref)
                    .and_then(|rack| rack.slots.get(slot))
                    .ok_or_else(|| {
                        format!("Track {} rack slot {} not found", track + 1, slot + 1)
                    })?;
                if rack_slot.effect_slots.len() != rack_slot.effect_descriptors.len() {
                    return Err(format!(
                        "Track {} rack slot {} FX state is misaligned: {} slots, {} descriptors",
                        track + 1,
                        slot + 1,
                        rack_slot.effect_slots.len(),
                        rack_slot.effect_descriptors.len()
                    ));
                }
                Ok(FxChainHost {
                    locator,
                    label: format!("Track {} Rack Slot {}", track + 1, slot + 1),
                    predecessor: StereoEndpoint {
                        node_id: slot_nodes.slot_pan_id,
                        channels: 2,
                    },
                    successor: ChainSuccessor::MonoPair {
                        left: track_nodes.voice_sum_id,
                        right: track_nodes.voice_sum_r_id,
                    },
                    slots: rack_slot
                        .effect_slots
                        .iter()
                        .zip(&rack_slot.effect_descriptors)
                        .map(|(effect, descriptor)| FxChainSlotView {
                            node_id: effect.node_id as i32,
                            modulator_node_id: effect.modulator_node_id as i32,
                            input_channels: descriptor.input_channels,
                            output_channels: descriptor.output_channels,
                        })
                        .collect(),
                })
            }
        }
    }

    fn fx_slot_identity(&self, locator: FxChainLocator, slot_idx: usize) -> Result<usize, String> {
        let (tag, payload) = match locator {
            FxChainLocator::Track(track) => {
                let effect_slot = slot_idx
                    .checked_sub(crate::effects::BUILTIN_SLOT_COUNT)
                    .ok_or_else(|| {
                        "Built-in track FX slots are not chain-host slots".to_string()
                    })?;
                let payload = track
                    .checked_mul(lisp_host::MAX_CUSTOM_FX)
                    .and_then(|base| base.checked_add(effect_slot))
                    .ok_or_else(|| "Track FX slot identity overflow".to_string())?;
                (0usize, payload)
            }
            FxChainLocator::Bus(bus_id) => {
                let bus = usize::try_from(bus_id.0)
                    .map_err(|_| format!("Bus id {} is too large for this platform", bus_id.0))?;
                let payload = bus
                    .checked_mul(lisp_host::MAX_CUSTOM_FX)
                    .and_then(|base| base.checked_add(slot_idx))
                    .ok_or_else(|| {
                        format!("Effect slot identity overflow for bus id {}", bus_id.0)
                    })?;
                (1usize, payload)
            }
            FxChainLocator::RackSlot { track, slot } => {
                let payload = track
                    .checked_mul(crate::sequencer::MAX_RACK_SLOTS)
                    .and_then(|base| base.checked_add(slot))
                    .and_then(|host| host.checked_mul(lisp_host::MAX_CUSTOM_FX))
                    .and_then(|base| base.checked_add(slot_idx))
                    .ok_or_else(|| "Rack-slot FX identity overflow".to_string())?;
                (2usize, payload)
            }
        };
        payload
            .checked_mul(3)
            .and_then(|value| value.checked_add(tag))
            .ok_or_else(|| "FX slot identity overflow".to_string())
    }

    pub(super) fn resolve_fx_slot(
        &self,
        locator: FxChainLocator,
        slot_idx: usize,
    ) -> Result<(usize, i32, usize, i32, usize, Option<i32>), String> {
        let host_slot = self.fx_slot_identity(locator, slot_idx)?;
        let wiring = self.fx_chain_host(locator)?.wiring_for(slot_idx)?;
        let successor = wiring.stereo_successor()?;
        Ok((
            host_slot,
            wiring.predecessor.node_id,
            wiring.predecessor.channels,
            successor.node_id,
            successor.channels,
            wiring.existing_node_id,
        ))
    }

    pub(super) fn remove_fx_slot_node(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        lease_removal: FxLeaseSlotRemoval,
    ) -> Result<(), String> {
        let host = self.fx_chain_host(locator)?;
        let wiring = host.wiring_for(slot_idx)?;
        let slot = host.slots.get(slot_idx).copied().ok_or_else(|| {
            format!(
                "{} effect slot {} is out of range",
                host.label,
                slot_idx + 1
            )
        })?;
        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        if slot.node_id > 0 {
            unsafe {
                let successor = match wiring.successor {
                    ChainSuccessor::StereoNode(successor) => {
                        lisp_host::EffectChainSuccessor::StereoNode {
                            node_id: successor.node_id,
                            input_channels: successor.channels,
                        }
                    }
                    ChainSuccessor::MonoPair { left, right } => {
                        lisp_host::EffectChainSuccessor::MonoPair { left, right }
                    }
                };
                lisp_host::remove_effect_from_chain_at_successor(
                    self.graph.lg.0,
                    slot.node_id,
                    wiring.predecessor.node_id,
                    successor,
                );
                if slot.modulator_node_id > 0 {
                    lisp_host::remove_effect_modulator(self.graph.lg.0, slot.modulator_node_id);
                }
            }
            crate::effects::conv_reverb::clear_instance(slot.node_id);
            connect_fx_chain_gap(self.graph.lg.0, wiring.predecessor, wiring.successor);
        }
        let applied =
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(self.graph.lg.0) };
        self.editor.effect_chain_leases.reclaim_applied(applied);
        match lease_removal {
            FxLeaseSlotRemoval::Clear => {
                self.editor
                    .effect_chain_leases
                    .set(locator, slot_idx, None, batch.serial)?
            }
            FxLeaseSlotRemoval::Shift => {
                self.editor
                    .effect_chain_leases
                    .remove_slot(locator, slot_idx, batch.serial)?
            }
        }
        Ok(())
    }

    pub(super) fn install_compiled_fx_node(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        result: lisp_host::CompileResult,
    ) -> Result<(lisp_host::DGenManifest, lisp_host::EffectGraphNodeIds), String> {
        let lisp_host::CompileResult {
            manifest,
            lib,
            lease,
        } = result;
        let slot_id = self.fx_slot_identity(locator, slot_idx)?;
        let host = self.fx_chain_host(locator)?;
        let wiring = host.wiring_for(slot_idx)?;
        let existing = wiring.existing_node_id;
        let existing_modulator = host
            .slots
            .get(slot_idx)
            .map(|slot| slot.modulator_node_id)
            .filter(|node_id| *node_id > 0);
        let ext_mod_inputs = self.fx_ext_mod_input_nodes(locator);
        let successor = match wiring.successor {
            ChainSuccessor::StereoNode(successor) => lisp_host::EffectChainSuccessor::StereoNode {
                node_id: successor.node_id,
                input_channels: successor.channels,
            },
            ChainSuccessor::MonoPair { left, right } => {
                lisp_host::EffectChainSuccessor::MonoPair { left, right }
            }
        };
        let node_ids = unsafe {
            lisp_host::add_effect_to_chain_at_successor(
                self.graph.lg.0,
                slot_id,
                &manifest,
                &lib,
                wiring.predecessor.node_id,
                wiring.predecessor.channels,
                successor,
                existing,
                existing_modulator,
                ext_mod_inputs.as_ref(),
            )
        }?;
        if let Some(old_node_id) = existing {
            crate::effects::conv_reverb::clear_instance(old_node_id);
        }
        self.editor.lisp_libs.push(lib);
        let applied =
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(self.graph.lg.0) };
        self.editor.effect_chain_leases.reclaim_applied(applied);
        self.editor.effect_chain_leases.set(
            locator,
            slot_idx,
            lease,
            node_ids.replacement_batch_serial,
        )?;
        Ok((manifest, node_ids))
    }

    pub(super) fn install_builtin_fx_node(
        &mut self,
        locator: FxChainLocator,
        slot_idx: usize,
        desc: &crate::effects::EffectDescriptor,
    ) -> Result<(i32, Option<i32>), String> {
        let slot_id = self.fx_slot_identity(locator, slot_idx)?;
        let host = self.fx_chain_host(locator)?;
        let wiring = host.wiring_for(slot_idx)?;
        let existing = wiring.existing_node_id;
        let existing_modulator = host
            .slots
            .get(slot_idx)
            .map(|slot| slot.modulator_node_id)
            .filter(|node_id| *node_id > 0);
        let ext_mod_inputs = self.fx_ext_mod_input_nodes(locator);
        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        let node_id = self.create_builtin_effect_node(slot_id, desc)?;
        let modulator_node_id = if desc.instrument_modulation_targets.is_empty() {
            None
        } else {
            Some(self.create_effect_modulator_node(&desc.name, slot_id)?)
        };
        unsafe {
            if let Some(old_id) = existing {
                let successor = match wiring.successor {
                    ChainSuccessor::StereoNode(successor) => {
                        lisp_host::EffectChainSuccessor::StereoNode {
                            node_id: successor.node_id,
                            input_channels: successor.channels,
                        }
                    }
                    ChainSuccessor::MonoPair { left, right } => {
                        lisp_host::EffectChainSuccessor::MonoPair { left, right }
                    }
                };
                lisp_host::remove_effect_from_chain_at_successor(
                    self.graph.lg.0,
                    old_id,
                    wiring.predecessor.node_id,
                    successor,
                );
                crate::effects::conv_reverb::clear_instance(old_id);
            }
            if let Some(old_modulator) = existing_modulator {
                lisp_host::remove_effect_modulator(self.graph.lg.0, old_modulator);
            }
            disconnect_fx_chain_gap(self.graph.lg.0, wiring.predecessor, wiring.successor);
            connect_fx_chain_gap(
                self.graph.lg.0,
                wiring.predecessor,
                ChainSuccessor::StereoNode(StereoEndpoint {
                    node_id,
                    channels: desc.input_channels,
                }),
            );
            connect_fx_chain_gap(
                self.graph.lg.0,
                StereoEndpoint {
                    node_id,
                    channels: desc.output_channels,
                },
                wiring.successor,
            );
            if let Some(modulator_id) = modulator_node_id {
                self.connect_effect_modulator_for_descriptor(
                    modulator_id,
                    node_id,
                    desc,
                    ext_mod_inputs.as_ref(),
                )?;
            }
        }
        let applied =
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(self.graph.lg.0) };
        self.editor.effect_chain_leases.reclaim_applied(applied);
        self.editor
            .effect_chain_leases
            .set(locator, slot_idx, None, batch.serial)?;
        Ok((node_id, modulator_node_id))
    }
}

pub(super) fn adapted_audio_port_connections(
    source_channels: usize,
    destination_channels: usize,
) -> Vec<(i32, i32)> {
    let source_channels = source_channels.clamp(1, 2);
    let destination_channels = destination_channels.clamp(1, 2);
    match (source_channels, destination_channels) {
        (1, 2) => vec![(0, 0), (0, 1)],
        (2, 1) => vec![(0, 0), (1, 0)],
        _ => (0..source_channels.min(destination_channels))
            .map(|channel| (channel as i32, channel as i32))
            .collect(),
    }
}

pub(super) fn push_fx_param(
    lg: *mut crate::audiograph::LiveGraph,
    node_id: u32,
    modulator_node_id: u32,
    node_param_idx: u32,
    node_param_span: u32,
    value: f32,
) {
    let target = if node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        (modulator_node_id != 0).then_some((
            modulator_node_id as u64,
            node_param_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
        ))
    } else if node_id != 0 && node_param_idx != u32::MAX {
        Some((node_id as u64, node_param_idx as u64))
    } else {
        None
    };
    let Some((logical_id, first_param_idx)) = target else {
        return;
    };
    unsafe {
        for lane in 0..node_param_span.max(1) as u64 {
            crate::audiograph::params_push_wrapper(
                lg,
                crate::audiograph::ParamMsg {
                    logical_id,
                    idx: first_param_idx + lane,
                    fvalue: value,
                },
            );
        }
    }
}

pub(super) fn connect_fx_chain_gap(
    lg: *mut crate::audiograph::LiveGraph,
    predecessor: StereoEndpoint,
    successor: ChainSuccessor,
) {
    let predecessor = predecessor.normalized();
    unsafe {
        match successor {
            ChainSuccessor::StereoNode(successor) => {
                for (source_port, destination_port) in
                    adapted_audio_port_connections(predecessor.channels, successor.channels)
                {
                    let _ = crate::audiograph::graph_connect(
                        lg,
                        predecessor.node_id,
                        source_port,
                        successor.node_id,
                        destination_port,
                    );
                }
            }
            ChainSuccessor::MonoPair { left, right } => {
                let right_source_port = if predecessor.channels > 1 { 1 } else { 0 };
                let _ = crate::audiograph::graph_connect(lg, predecessor.node_id, 0, left, 0);
                let _ = crate::audiograph::graph_connect(
                    lg,
                    predecessor.node_id,
                    right_source_port,
                    right,
                    0,
                );
            }
        }
    }
}

fn disconnect_fx_chain_gap(
    lg: *mut crate::audiograph::LiveGraph,
    predecessor: StereoEndpoint,
    successor: ChainSuccessor,
) {
    unsafe {
        match successor {
            ChainSuccessor::StereoNode(successor) => {
                for source_port in 0..2 {
                    for destination_port in 0..2 {
                        let _ = crate::audiograph::graph_disconnect(
                            lg,
                            predecessor.node_id,
                            source_port,
                            successor.node_id,
                            destination_port,
                        );
                    }
                }
            }
            ChainSuccessor::MonoPair { left, right } => {
                for source_port in 0..2 {
                    let _ = crate::audiograph::graph_disconnect(
                        lg,
                        predecessor.node_id,
                        source_port,
                        left,
                        0,
                    );
                    let _ = crate::audiograph::graph_disconnect(
                        lg,
                        predecessor.node_id,
                        source_port,
                        right,
                        0,
                    );
                }
            }
        }
    }
}

pub(super) fn rewire_fx_chain(
    lg: *mut crate::audiograph::LiveGraph,
    previous: &FxChainHost,
    current: &FxChainHost,
) {
    for (predecessor, successor) in previous.connections() {
        disconnect_fx_chain_gap(lg, predecessor, successor);
    }
    for (predecessor, successor) in current.connections() {
        disconnect_fx_chain_gap(lg, predecessor, successor);
        connect_fx_chain_gap(lg, predecessor, successor);
    }
}

pub(super) fn connect_fx_chain_host(lg: *mut crate::audiograph::LiveGraph, host: &FxChainHost) {
    disconnect_fx_chain_gap(lg, host.predecessor, host.successor);
    for (predecessor, successor) in host.connections() {
        disconnect_fx_chain_gap(lg, predecessor, successor);
        connect_fx_chain_gap(lg, predecessor, successor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(node_id: i32, input_channels: usize, output_channels: usize) -> FxChainSlotView {
        FxChainSlotView {
            node_id,
            modulator_node_id: 0,
            input_channels,
            output_channels,
        }
    }

    #[test]
    fn lease_slot_insert_shifts_right_and_drops_tail() {
        let mut row = vec![Some(1), Some(2), None, Some(4)];
        insert_empty_slot(&mut row, 1);
        assert_eq!(row, vec![Some(1), None, Some(2), None]);
    }

    #[test]
    fn lease_slot_remove_shifts_left_and_clears_tail() {
        let mut row = vec![Some(1), Some(2), Some(3), None];
        assert_eq!(remove_slot(&mut row, 1), Some(2));
        assert_eq!(row, vec![Some(1), Some(3), None, None]);
    }

    #[test]
    fn lease_slot_move_preserves_relative_order() {
        let mut row = vec![Some(1), Some(2), Some(3), Some(4)];
        move_slot(&mut row, 0, 2);
        assert_eq!(row, vec![Some(2), Some(3), Some(1), Some(4)]);
        move_slot(&mut row, 3, 1);
        assert_eq!(row, vec![Some(2), Some(4), Some(3), Some(1)]);
    }

    #[test]
    fn audio_port_adapter_duplicates_mono_and_folds_stereo() {
        assert_eq!(adapted_audio_port_connections(1, 1), vec![(0, 0)]);
        assert_eq!(adapted_audio_port_connections(1, 2), vec![(0, 0), (0, 1)]);
        assert_eq!(adapted_audio_port_connections(2, 1), vec![(0, 0), (1, 0)]);
        assert_eq!(adapted_audio_port_connections(2, 2), vec![(0, 0), (1, 1)]);
        assert_eq!(adapted_audio_port_connections(4, 4), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn host_wiring_skips_empty_slots_and_preserves_channel_counts() {
        let host = FxChainHost {
            locator: FxChainLocator::Track(0),
            label: "Track 1".to_string(),
            predecessor: StereoEndpoint {
                node_id: 10,
                channels: 2,
            },
            successor: ChainSuccessor::StereoNode(StereoEndpoint {
                node_id: 20,
                channels: 2,
            }),
            slots: vec![slot(30, 1, 1), slot(0, 1, 1), slot(40, 2, 2)],
        };

        let wiring = host.wiring_for(1).expect("middle slot wiring");
        assert_eq!(
            wiring.predecessor,
            StereoEndpoint {
                node_id: 30,
                channels: 1
            }
        );
        assert_eq!(
            wiring.successor,
            ChainSuccessor::StereoNode(StereoEndpoint {
                node_id: 40,
                channels: 2
            })
        );
        assert_eq!(wiring.existing_node_id, None);
    }

    #[test]
    fn rack_slot_host_terminates_stereo_chain_at_independent_mono_sums() {
        let host = FxChainHost {
            locator: FxChainLocator::RackSlot { track: 2, slot: 3 },
            label: "rack slot".to_string(),
            predecessor: StereoEndpoint {
                node_id: 10,
                channels: 2,
            },
            successor: ChainSuccessor::MonoPair {
                left: 20,
                right: 21,
            },
            slots: vec![slot(30, 2, 2), slot(0, 2, 2), slot(40, 1, 1)],
        };
        assert_eq!(
            host.connections(),
            vec![
                (
                    StereoEndpoint {
                        node_id: 10,
                        channels: 2
                    },
                    ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: 30,
                        channels: 2
                    })
                ),
                (
                    StereoEndpoint {
                        node_id: 30,
                        channels: 2
                    },
                    ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: 40,
                        channels: 1
                    })
                ),
                (
                    StereoEndpoint {
                        node_id: 40,
                        channels: 1
                    },
                    ChainSuccessor::MonoPair {
                        left: 20,
                        right: 21
                    }
                ),
            ]
        );
    }

    #[test]
    fn rack_slot_lease_hosts_follow_delete_and_reorder() {
        let mut store = FxChainLeaseStore::default();
        store.row_mut(FxChainLocator::RackSlot { track: 1, slot: 0 });
        store.row_mut(FxChainLocator::RackSlot { track: 1, slot: 1 });
        store.row_mut(FxChainLocator::RackSlot { track: 1, slot: 2 });
        store.reindex_rack_slots_after_delete(1, 1);
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 0 }));
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 1 }));
        assert!(!store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 2 }));

        store.move_rack_slot_host(1, 0, 1);
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 0 }));
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 1 }));
    }

    #[test]
    fn host_wiring_uses_terminal_endpoints_at_chain_edges() {
        let host = FxChainHost {
            locator: FxChainLocator::Track(0),
            label: "Track 1".to_string(),
            predecessor: StereoEndpoint {
                node_id: 10,
                channels: 2,
            },
            successor: ChainSuccessor::MonoPair {
                left: 21,
                right: 22,
            },
            slots: vec![slot(0, 2, 2), slot(0, 2, 2)],
        };

        let wiring = host.wiring_for(0).expect("first slot wiring");
        assert_eq!(wiring.predecessor, host.predecessor);
        assert_eq!(wiring.successor, host.successor);
    }

    #[test]
    fn host_connections_describe_the_complete_sparse_chain() {
        let host = FxChainHost {
            locator: FxChainLocator::Track(0),
            label: "Track 1".to_string(),
            predecessor: StereoEndpoint {
                node_id: 10,
                channels: 2,
            },
            successor: ChainSuccessor::StereoNode(StereoEndpoint {
                node_id: 20,
                channels: 2,
            }),
            slots: vec![slot(30, 1, 1), slot(0, 2, 2), slot(40, 2, 2)],
        };

        assert_eq!(
            host.connections(),
            vec![
                (
                    StereoEndpoint {
                        node_id: 10,
                        channels: 2,
                    },
                    ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: 30,
                        channels: 1,
                    }),
                ),
                (
                    StereoEndpoint {
                        node_id: 30,
                        channels: 1,
                    },
                    ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: 40,
                        channels: 2,
                    }),
                ),
                (
                    StereoEndpoint {
                        node_id: 40,
                        channels: 2,
                    },
                    ChainSuccessor::StereoNode(StereoEndpoint {
                        node_id: 20,
                        channels: 2,
                    }),
                ),
            ]
        );
    }

    #[test]
    fn track_lease_slots_use_chain_relative_indices() {
        assert_eq!(
            FxChainLeaseStore::row_slot(
                FxChainLocator::Track(3),
                crate::effects::BUILTIN_SLOT_COUNT,
            ),
            Ok(0),
        );
        assert_eq!(
            FxChainLeaseStore::row_slot(
                FxChainLocator::Track(3),
                crate::effects::BUILTIN_SLOT_COUNT + lisp_host::MAX_CUSTOM_FX - 1,
            ),
            Ok(lisp_host::MAX_CUSTOM_FX - 1),
        );
        if crate::effects::BUILTIN_SLOT_COUNT > 0 {
            assert!(
                FxChainLeaseStore::row_slot(FxChainLocator::Track(3), 0).is_err(),
                "built-in slots must not alias custom-effect leases",
            );
        }
    }

    #[test]
    fn deleting_a_track_rekeys_later_track_lease_hosts() {
        let mut store = FxChainLeaseStore::default();
        store.rows.insert(
            FxChainLocator::Track(3),
            std::iter::repeat_with(|| None)
                .take(lisp_host::MAX_CUSTOM_FX)
                .collect(),
        );

        store.reindex_tracks_after_delete(1);

        assert!(!store.contains_host(FxChainLocator::Track(3)));
        assert!(store.contains_host(FxChainLocator::Track(2)));
    }

    #[test]
    fn inserting_appended_track_rekeys_track_and_rack_lease_hosts() {
        let mut store = FxChainLeaseStore::default();
        for locator in [
            FxChainLocator::Track(1),
            FxChainLocator::Track(2),
            FxChainLocator::RackSlot { track: 1, slot: 0 },
            FxChainLocator::RackSlot { track: 2, slot: 0 },
        ] {
            store.rows.insert(
                locator,
                std::iter::repeat_with(|| None)
                    .take(lisp_host::MAX_CUSTOM_FX)
                    .collect(),
            );
        }

        store.reindex_tracks_move_last_to(2, 1);

        assert!(store.contains_host(FxChainLocator::Track(1)));
        assert!(store.contains_host(FxChainLocator::Track(2)));
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 1, slot: 0 }));
        assert!(store.contains_host(FxChainLocator::RackSlot { track: 2, slot: 0 }));
    }

    #[test]
    fn replaced_lease_is_held_until_its_graph_batch_is_applied() {
        let manager = crate::lisp_host::dylib_cache::DylibCacheManager::workspace_default();
        let artifact = std::path::PathBuf::from("fx-chain-retirement-test-artifact");
        let lease = manager.test_lease(&artifact);
        let locator = FxChainLocator::Track(0);
        let mut store = FxChainLeaseStore::default();
        store
            .set(locator, crate::effects::BUILTIN_SLOT_COUNT, Some(lease), 0)
            .expect("initial lease install should not require retirement");

        store
            .set(locator, crate::effects::BUILTIN_SLOT_COUNT, None, 42)
            .expect("replacement should retain the old lease under batch 42");
        assert_eq!(manager.live_lease_count(&artifact), 1);

        store.reclaim_applied(41);
        assert_eq!(manager.live_lease_count(&artifact), 1);

        store.reclaim_applied(42);
        assert_eq!(manager.live_lease_count(&artifact), 0);
    }
}
