//! Drop-to-swap scoped to the viewed scene bank (docs/rig-spec.md §4,
//! eseq-ka5t.9).
//!
//! One rule: a drop changes only the viewed bank. Scene state
//! (`Scene::slot_enable`) says which rack slots each scene plays, so a swap
//! writes the bank's scenes and leaves every other bank playing what it
//! played. Patterns change only by gaining a slot (same id in every pattern)
//! or by a slot's device being overwritten in every pattern (a slot id is one
//! device, §3.2). A sample dropped on a sampler slot is content, not a
//! device, and keeps the per-pattern replace.
//!
//! Every gesture here is one undo step: the instrument-binding history entry
//! captures the whole rack container (every pattern's slots, the slot FX, the
//! device identities) together with every scene's `slot_enable` for the
//! track.

use std::ops::Range;
use std::path::{Path, PathBuf};

use super::effects::PreparedRackInstrument;
use super::fx_chain::RetainedEffectSource;
use super::graph::{custom_rack_slot_snapshot, sampler_rack_slot_snapshot, RackSlotReplaceScope};
use super::App;
use crate::sequencer::{InstrumentType, RackSlotSnapshot, SceneBankId};

#[cfg(test)]
mod tests;

/// Which scenes a swap writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapScope {
    /// The scenes of a bank (`None`: the bank holding the playing scene).
    Bank(Option<SceneBankId>),
    /// "Replace in all banks": overwrite the slot's device in every pattern,
    /// whatever other banks play.
    AllBanks,
}

/// What a swap stored (rig spec §4 "replace for this bank").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapKind {
    /// A sample onto a sampler slot: that pattern's sample only.
    Content,
    /// The slot's device was overwritten in every pattern, same id.
    Overwritten,
    /// Another bank plays the old slot: a new slot was inserted after it.
    Inserted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapOutcome {
    /// The slot position now holding the dropped device.
    pub slot: usize,
    pub kind: SwapKind,
    /// The banks that kept the old device ("Kept Piano for banks A, B").
    pub kept_banks: Vec<SceneBankId>,
    /// The old device's display name.
    pub old_name: String,
    /// A plain track was converted to an Instrument Rack first.
    pub converted: bool,
}

impl SwapOutcome {
    /// A sample onto a sampler slot (or a sampler already there): content.
    pub fn content(slot: usize, old_name: String) -> Self {
        Self::unconverted(slot, SwapKind::Content, Vec::new(), old_name)
    }

    /// The slot's device overwritten in every pattern, same id.
    pub fn overwritten(slot: usize, old_name: String) -> Self {
        Self::unconverted(slot, SwapKind::Overwritten, Vec::new(), old_name)
    }

    fn unconverted(
        slot: usize,
        kind: SwapKind,
        kept_banks: Vec<SceneBankId>,
        old_name: String,
    ) -> Self {
        Self {
            slot,
            kind,
            kept_banks,
            old_name,
            converted: false,
        }
    }
}

/// A sample decoded into a graph buffer for a rack slot, not yet
/// registered: registration (the path the project saves) and the onset
/// analysis wait until the slot edit succeeded, so a failed swap queues no
/// work for a buffer nothing plays. (Graph sample buffers are never freed,
/// `project_import/assets.rs`; a swap checks what it can before decoding.)
#[derive(Clone)]
pub(crate) struct RackSample {
    pub buffer_id: i32,
    pub sample_rate: u32,
    pub name: String,
    pub path: Option<PathBuf>,
    /// The decoded mono samples to analyse on registration (a silent
    /// sampler has none).
    pub mono_samples: Option<std::sync::Arc<Vec<f32>>>,
}

/// A device a drop puts in a slot.
#[derive(Clone)]
pub(crate) enum SlotDevice {
    Sampler(RackSample),
    Instrument(PreparedRackInstrument),
}

impl SlotDevice {
    fn snapshot(&self) -> RackSlotSnapshot {
        match self {
            Self::Sampler(sample) => {
                sampler_rack_slot_snapshot(sample.buffer_id, sample.sample_rate, &sample.name)
            }
            Self::Instrument(prepared) => custom_rack_slot_snapshot(
                &prepared.name,
                prepared.engine_id,
                &prepared.manifest,
                prepared.run_mode,
            ),
        }
    }
}

impl App {
    /// `bank`'s id and scene range (`None`: the playing scene's bank),
    /// `ProjectScenes::resolve_bank_range`.
    pub fn bank_scene_range(
        &self,
        bank: Option<SceneBankId>,
    ) -> Result<(SceneBankId, Range<usize>), String> {
        self.state
            .with_project_scenes(|scenes| scenes.resolve_bank_range(bank))
    }

    /// Decode the sample at `path` into a graph buffer and name it: the
    /// shared first half of every sample-to-rack-slot load (add, per-pattern
    /// replace, bank swap).
    pub(crate) fn load_rack_sample(&mut self, path: &Path) -> Result<RackSample, String> {
        let loaded = crate::instruments::sampler::load_wav_buffer(self.graph.lg.0, path)?;
        let name = crate::sample_db::display_title_for_sample_path(path).unwrap_or(loaded.name);
        Ok(RackSample {
            buffer_id: loaded.buffer_id,
            sample_rate: loaded.sample_rate,
            name,
            path: Some(path.to_path_buf()),
            mono_samples: Some(std::sync::Arc::new(loaded.mono_samples)),
        })
    }

    /// The second half, once the slot holds it: queue the buffer's
    /// analysis and remember the file it came from.
    pub(crate) fn register_rack_sample(&mut self, sample: &RackSample) {
        if let Some(samples) = &sample.mono_samples {
            self.sample_analysis.submit(crate::analysis::AnalysisJob::new(
                sample.buffer_id,
                std::sync::Arc::clone(samples),
                sample.sample_rate,
            ));
        }
        if let Some(path) = &sample.path {
            self.register_loaded_sample_path(&sample.name, sample.buffer_id, path.clone());
        }
    }

    fn register_slot_device(&mut self, device: &SlotDevice) {
        if let SlotDevice::Sampler(sample) = device {
            self.register_rack_sample(sample);
        }
    }

    /// Switch the slot just added at `slot_idx` on in the scenes of `range`
    /// only (rig spec §4 "add": it exists, disabled, in every other scene).
    fn scope_added_rack_slot_to(&self, track: usize, slot_idx: usize, range: Range<usize>) {
        if let Some(slot_id) = self.state.live_rack_slot_id(track, slot_idx) {
            self.state
                .set_rack_slot_enabled_where(track, slot_id, |scene, _| {
                    Some(range.contains(&scene))
                });
        }
    }

    /// Add a sampler slot holding `path` to rack `track`, enabled in the
    /// scenes of `bank` only, as one undo step (the empty slot row).
    pub fn add_rack_sample_slot_in_bank_recorded(
        &mut self,
        track: usize,
        path: &Path,
        bank: Option<SceneBankId>,
    ) -> Result<usize, String> {
        let (_, range) = self.bank_scene_range(bank)?;
        self.apply_recorded_rack_slot_add(track, "Add rack sample", |app| {
            let slot_idx = app
                .graph_controller()
                .add_sampler_slot_to_rack(track, path)?;
            app.scope_added_rack_slot_to(track, slot_idx, range);
            Ok(slot_idx)
        })
    }

    /// Add saved instrument `name` as a slot of rack `track`, enabled in the
    /// scenes of `bank` only, as one undo step.
    pub fn add_saved_instrument_slot_to_rack_in_bank_sync(
        &mut self,
        track: usize,
        name: &str,
        bank: Option<SceneBankId>,
    ) -> Result<usize, String> {
        let (_, range) = self.bank_scene_range(bank)?;
        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        let lib_ptr: *const crate::lisp_host::LoadedDGenLib =
            &self.editor.instrument_libs[prepared.lib_index];
        self.apply_recorded_rack_slot_add(track, "Add rack instrument", |app| {
            let slot_idx = unsafe {
                app.graph_controller().add_custom_slot_to_rack(
                    track,
                    &prepared.name,
                    prepared.engine_id,
                    &prepared.manifest,
                    &*lib_ptr,
                    prepared.run_mode,
                )
            }?;
            app.scope_added_rack_slot_to(track, slot_idx, range);
            Ok(slot_idx)
        })
    }

    /// Drop saved instrument `name` on rack slot `slot` of `track`.
    pub fn swap_rack_slot_to_saved_instrument(
        &mut self,
        track: usize,
        slot: usize,
        name: &str,
        scope: SwapScope,
    ) -> Result<SwapOutcome, String> {
        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        self.swap_rack_slot_recorded(track, slot, SlotDevice::Instrument(prepared), scope)
    }

    /// Drop the sample at `path` on rack slot `slot` of `track`. Onto a
    /// sampler slot it is content: that pattern's sample (a kit pad's: that
    /// clip's), as before rev 3.
    pub fn swap_rack_slot_to_sample(
        &mut self,
        track: usize,
        slot: usize,
        path: &Path,
        scope: SwapScope,
    ) -> Result<SwapOutcome, String> {
        let old = self
            .state
            .live_rack_track_snapshot(track)
            .and_then(|rack| rack.slots.get(slot).cloned())
            .ok_or_else(|| format!("Track {} has no rack slot {}", track + 1, slot + 1))?;
        if old.instrument_type == InstrumentType::Sampler && scope != SwapScope::AllBanks {
            let old_name = self.rack_slot_display_name(track, slot);
            self.apply_recorded_rack_slot_source_replacement(
                track,
                slot,
                "Replace rack sample",
                |app| {
                    app.graph_controller()
                        .replace_rack_slot_with_sampler(track, slot, path)
                },
            )?;
            return Ok(SwapOutcome::content(slot, old_name));
        }
        // What can fail before the swap touches anything fails before the
        // decode (a decoded buffer is never freed).
        if let SwapScope::Bank(bank) = scope {
            self.bank_scene_range(bank)?;
        }
        let device = SlotDevice::Sampler(self.load_rack_sample(path)?);
        self.swap_rack_slot_recorded(track, slot, device, scope)
    }

    pub(crate) fn swap_rack_slot_recorded(
        &mut self,
        track: usize,
        slot: usize,
        device: SlotDevice,
        scope: SwapScope,
    ) -> Result<SwapOutcome, String> {
        let label = match scope {
            SwapScope::AllBanks => "Replace rack layer in all banks",
            SwapScope::Bank(_) => "Replace rack layer for this bank",
        };
        let outcome = self.apply_recorded_instrument_binding_mutation(track, label, |app| {
            app.swap_rack_slot(track, slot, &device, scope)
        })?;
        self.register_slot_device(&device);
        Ok(outcome)
    }

    /// The unrecorded swap of rack slot `slot` of `track` to `device`.
    fn swap_rack_slot(
        &mut self,
        track: usize,
        slot: usize,
        device: &SlotDevice,
        scope: SwapScope,
    ) -> Result<SwapOutcome, String> {
        let old_id = self
            .state
            .live_rack_slot_id(track, slot)
            .ok_or_else(|| format!("Track {} has no rack slot {}", track + 1, slot + 1))?;
        let old_name = self.rack_slot_display_name(track, slot);
        let snapshot = device.snapshot();
        let scope_range = match scope {
            SwapScope::AllBanks => None,
            SwapScope::Bank(bank) => Some(self.bank_scene_range(bank)?),
        };
        let Some((bank, range)) = scope_range else {
            self.graph_controller().replace_layer_rack_slot_source(
                track,
                slot,
                snapshot,
                RackSlotReplaceScope::AllPatterns,
            )?;
            return Ok(SwapOutcome::overwritten(slot, old_name));
        };
        // The bank's scenes the new device plays in: those that played the
        // old one, or the whole bank when none did (a drop on a disabled row
        // still means "play this here").
        let (targets, outside) = self.state.with_project_scenes(|scenes| {
            let mut targets: Vec<usize> = range
                .clone()
                .filter(|scene| scenes.slot_enabled_in_scene(*scene, track, old_id) == Some(true))
                .collect();
            if targets.is_empty() {
                targets = range.clone().collect();
            }
            (
                targets,
                scenes.rack_slot_enabled_outside(track, old_id, range.clone()),
            )
        });
        if !outside {
            self.graph_controller().replace_layer_rack_slot_source(
                track,
                slot,
                snapshot,
                RackSlotReplaceScope::AllPatterns,
            )?;
            self.state
                .set_rack_slot_enabled_where(track, old_id, |scene, _| {
                    targets.contains(&scene).then_some(true)
                });
            return Ok(SwapOutcome::overwritten(slot, old_name));
        }
        let position = slot + 1;
        self.graph_controller()
            .insert_rack_slot(track, position, snapshot, Some(slot))?;
        self.copy_rack_slot_effects(track, slot, position)?;
        let new_id = self
            .state
            .live_rack_slot_id(track, position)
            .ok_or_else(|| "The inserted rack layer has no id".to_string())?;
        self.state
            .set_rack_slot_enabled_where(track, new_id, |scene, _| {
                range.contains(&scene).then(|| targets.contains(&scene))
            });
        self.state
            .set_rack_slot_enabled_where(track, old_id, |scene, _| {
                range.contains(&scene).then_some(false)
            });
        // The copied effects bypass wherever the playing scene parks their
        // slot (eseq-bw9v), from the first block.
        self.state.publish_scheduler_snapshot();
        self.push_rack_slot_fx_gate(track, slot);
        self.push_rack_slot_fx_gate(track, position);
        let kept_banks = self
            .state
            .banks_enabling_rack_slot(track, old_id)
            .into_iter()
            .filter(|kept| *kept != bank)
            .collect();
        Ok(SwapOutcome::unconverted(
            position,
            SwapKind::Inserted,
            kept_banks,
            old_name,
        ))
    }

    /// Give rack slot `to` its own instances of every effect rack slot
    /// `from` runs, in the same chain positions, with `from`'s values
    /// (params, bypass, p-locks) pattern by pattern: "replace for this bank"
    /// keeps the slot's effects, as an in-place overwrite does. Each copy is
    /// a new effect node with its own lease and device identity.
    fn copy_rack_slot_effects(
        &mut self,
        track: usize,
        from: usize,
        to: usize,
    ) -> Result<(), String> {
        let rack = self
            .state
            .live_rack_track_snapshot(track)
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        let source_slot = rack
            .slots
            .get(from)
            .ok_or_else(|| format!("Rack slot {} is missing", from + 1))?;
        let effects: Vec<(usize, String)> = source_slot
            .effect_slots
            .iter()
            .zip(&source_slot.effect_descriptors)
            .enumerate()
            .filter(|(_, (effect, _))| effect.node_id != 0)
            .map(|(effect_slot, (_, descriptor))| (effect_slot, descriptor.name.trim().to_string()))
            .collect();
        // Read before any copy loads: keyed by the source nodes.
        let prepared: std::collections::HashMap<usize, crate::effects::EffectSlotValuesSnapshot> =
            effects
                .iter()
                .map(|(effect_slot, _)| {
                    let effect = &source_slot.effect_slots[*effect_slot];
                    let node = effect.node_id as i32;
                    let mut values = effect.authoring_values();
                    // The loaded reference lives with the node when the
                    // snapshot has not recorded it yet.
                    values.ir = values
                        .ir
                        .or_else(|| crate::effects::conv_reverb::ir_ref_for(node));
                    values.table = values
                        .table
                        .or_else(|| crate::effects::filter_table::table_ref_for(node));
                    (*effect_slot, values)
                })
                .collect();
        let from_locator = Self::rack_slot_fx_locator(track, from);
        let to_locator = Self::rack_slot_fx_locator(track, to);
        for (effect_slot, name) in effects {
            let source = self
                .editor
                .effect_chain_leases
                .source(from_locator, effect_slot)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.retained_effect_source_for_name(&name))?;
            match &source {
                RetainedEffectSource::NativeBuiltin { name } => {
                    self.load_builtin_rack_slot_effect_to_slot_sync(track, to, effect_slot, name)?;
                }
                RetainedEffectSource::Compiled { name, .. }
                    if crate::effects::dgen_builtin::find(name).is_some() =>
                {
                    self.load_builtin_rack_slot_effect_to_slot_sync(track, to, effect_slot, name)?;
                }
                RetainedEffectSource::Compiled {
                    name,
                    source: code,
                    asset_base,
                    origin,
                } => {
                    let result = self.editor.dylib_cache.acquire(
                        crate::lisp_host::DGenCompileKind::Effect,
                        *origin,
                        code,
                        self.graph.sample_rate,
                        asset_base.as_deref(),
                    )?;
                    self.apply_compiled_rack_slot_effect_to_slot_sync(
                        track,
                        to,
                        effect_slot,
                        name,
                        result,
                    )?;
                }
            }
            self.retain_effect_source(to_locator, effect_slot, source.clone())?;
            let descriptor = self
                .rack_slot_effect_snapshot(track, to)?
                .effect_descriptors
                .get(effect_slot)
                .cloned()
                .ok_or_else(|| "Copied rack-slot effect has no descriptor".to_string())?;
            if !self
                .state
                .copy_rack_slot_effect_values(track, from, to, effect_slot, &descriptor)
            {
                return Err("Failed to copy rack-slot effect values".to_string());
            }
            self.push_rack_slot_effect_defaults(track, to, effect_slot);
            // The source node's decoded IR / filter table: the copy plays
            // through the same loaded data, not an empty reverb.
            self.restore_prepared_rack_effect_data(
                track,
                to,
                effect_slot,
                &prepared[&effect_slot],
            )?;
        }
        self.graph_controller()
            .refresh_rack_signature_from_live_state(track);
        Ok(())
    }

    /// Saved instrument `name` dropped on track `track` (its row or its
    /// instrument panel) under the bank rule (rig spec §4): a rack track
    /// another bank plays swaps the slot the bank plays; a plain sampler or
    /// synth track another bank plays becomes an Instrument Rack with its
    /// device as slot 1 (every scene keeps playing it), and the new device
    /// plays in the bank alone, as one undo step. `Ok(None)`: no other bank
    /// plays the track, so the caller replaces the instrument as before.
    pub fn swap_track_to_saved_instrument_for_bank(
        &mut self,
        track: usize,
        name: &str,
        bank: Option<SceneBankId>,
    ) -> Result<Option<SwapOutcome>, String> {
        if let Some(slot) = self.rack_track_bank_swap_slot(track, bank)? {
            return self
                .swap_rack_slot_to_saved_instrument(track, slot, name, SwapScope::Bank(bank))
                .map(Some);
        }
        if !self.plain_track_swap_needs_rack(track, bank)? {
            return Ok(None);
        }
        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        self.swap_plain_track_recorded(track, SlotDevice::Instrument(prepared), bank)
            .map(Some)
    }

    /// `swap_track_to_saved_instrument_for_bank` for a sample dropped on a
    /// track (on a plain sampler track it is content: `Ok(None)`).
    pub fn swap_track_to_sample_for_bank(
        &mut self,
        track: usize,
        path: &Path,
        bank: Option<SceneBankId>,
    ) -> Result<Option<SwapOutcome>, String> {
        // A rack track another bank plays: the playing slot, for the bank (a
        // sampler slot takes the sample as content, as a slot drop does).
        if let Some(slot) = self.rack_track_bank_swap_slot(track, bank)? {
            return self
                .swap_rack_slot_to_sample(track, slot, path, SwapScope::Bank(bank))
                .map(Some);
        }
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom)
            || !self.plain_track_swap_needs_rack(track, bank)?
        {
            return Ok(None);
        }
        let device = SlotDevice::Sampler(self.load_rack_sample(path)?);
        self.swap_plain_track_recorded(track, device, bank)
            .map(Some)
    }

    /// The builtin sampler with no sample (`swap-track-builtin-instrument`)
    /// under the same bank rule: a synth track another bank plays converts
    /// and gets a silent sampler for the bank; a rack track another bank
    /// plays swaps its playing slot. `Ok(None)`: the caller converts as
    /// before. A sampler already playing there is left as it is (`Content`).
    pub fn swap_track_to_builtin_sampler_for_bank(
        &mut self,
        track: usize,
        bank: Option<SceneBankId>,
    ) -> Result<Option<SwapOutcome>, String> {
        let rack_slot = self.rack_track_bank_swap_slot(track, bank)?;
        let plain = rack_slot.is_none()
            && self.graph.track_instrument_types.get(track) == Some(&InstrumentType::Custom)
            && self.plain_track_swap_needs_rack(track, bank)?;
        if rack_slot.is_none() && !plain {
            return Ok(None);
        }
        if let Some(slot) = rack_slot {
            let old = self
                .state
                .live_rack_track_snapshot(track)
                .and_then(|rack| rack.slots.get(slot).cloned());
            if old.is_some_and(|old| old.instrument_type == InstrumentType::Sampler) {
                return Ok(Some(SwapOutcome::content(
                    slot,
                    self.rack_slot_display_name(track, slot),
                )));
            }
        }
        let device = SlotDevice::Sampler(RackSample {
            buffer_id: crate::instruments::sampler::create_silent_buffer(self.graph.lg.0)?,
            sample_rate: self.graph.sample_rate,
            name: format!("Sampler {}", track + 1),
            path: None,
            mono_samples: None,
        });
        match rack_slot {
            Some(slot) => self
                .swap_rack_slot_recorded(track, slot, device, SwapScope::Bank(bank))
                .map(Some),
            None => self
                .swap_plain_track_recorded(track, device, bank)
                .map(Some),
        }
    }

    /// Whether a device drop on plain track `track` must convert it: it is a
    /// sampler or synth track and a scene outside `bank` gives it a cell.
    pub fn plain_track_swap_needs_rack(
        &self,
        track: usize,
        bank: Option<SceneBankId>,
    ) -> Result<bool, String> {
        if !matches!(
            self.graph.track_instrument_types.get(track),
            Some(InstrumentType::Custom | InstrumentType::Sampler)
        ) {
            return Ok(false);
        }
        let (_, range) = self.bank_scene_range(bank)?;
        Ok(self
            .state
            .with_project_scenes(|scenes| scenes.track_played_outside(track, range)))
    }

    /// A drop on an Instrument Rack track's row (not on a slot): when a scene
    /// outside `bank` plays the track, the slot the drop replaces for the bank
    /// — the first slot any scene of the bank plays, else slot 1 — so
    /// replacing the whole rack never touches another bank's devices. `None`:
    /// no other bank plays it (the caller replaces the instrument as before),
    /// or the track is no rack with slots.
    pub fn rack_track_bank_swap_slot(
        &self,
        track: usize,
        bank: Option<SceneBankId>,
    ) -> Result<Option<usize>, String> {
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Ok(None);
        }
        let ids: Vec<u64> = self
            .state
            .live_rack_track_snapshot(track)
            .map(|rack| rack.slots.iter().map(|slot| slot.id).collect())
            .unwrap_or_default();
        if ids.is_empty() {
            return Ok(None);
        }
        let (_, range) = self.bank_scene_range(bank)?;
        Ok(self.state.with_project_scenes(|scenes| {
            scenes.track_played_outside(track, range.clone()).then(|| {
                ids.iter()
                    .position(|id| {
                        range.clone().any(|scene| {
                            scenes.slot_enabled_in_scene(scene, track, *id) == Some(true)
                        })
                    })
                    .unwrap_or(0)
            })
        }))
    }

    fn swap_plain_track_recorded(
        &mut self,
        track: usize,
        device: SlotDevice,
        bank: Option<SceneBankId>,
    ) -> Result<SwapOutcome, String> {
        let mut outcome = self.group_track_to_instrument_rack_then_recorded(
            track,
            "Replace instrument for this bank",
            |app| app.swap_rack_slot(track, 0, &device, SwapScope::Bank(bank)),
        )?;
        outcome.converted = true;
        self.register_slot_device(&device);
        Ok(outcome)
    }

    /// "Use only this in bank X" (rig spec §4 recall): slot `slot` plays in
    /// every scene of `bank` and every other slot of the track stops there,
    /// as one undo step. Returns whether anything changed.
    pub fn use_only_rack_slot_in_bank_recorded(
        &mut self,
        track: usize,
        slot: usize,
        bank: Option<SceneBankId>,
    ) -> Result<bool, String> {
        let (_, range) = self.bank_scene_range(bank)?;
        let ids: Vec<u64> = self
            .state
            .live_rack_track_snapshot(track)
            .map(|rack| rack.slots.iter().map(|slot| slot.id).collect())
            .unwrap_or_default();
        let chosen = *ids
            .get(slot)
            .ok_or_else(|| format!("Track {} has no rack slot {}", track + 1, slot + 1))?;
        let changes = self.state.with_project_scenes(|scenes| {
            ids.iter().any(|id| {
                scenes.rack_slot_enable_would_change(track, *id, *id == chosen, range.clone())
            })
        });
        if !changes {
            return Ok(false);
        }
        let changed =
            self.apply_recorded_scene_structure_mutation("Use rack slot in bank", |app| {
                let mut changed = false;
                for id in &ids {
                    let on = *id == chosen;
                    changed |= app
                        .state
                        .set_rack_slot_enabled_where(track, *id, |scene, _| {
                            range.contains(&scene).then_some(on)
                        });
                }
                Ok(changed)
            })?;
        self.state.publish_scheduler_snapshot();
        Ok(changed)
    }

    /// The display names of `track`'s slots no scene enables (rig spec §4
    /// purge), with their positions.
    pub fn unused_rack_slot_names(&self, track: usize) -> Vec<(usize, String)> {
        self.state
            .unused_rack_slots(track)
            .into_iter()
            .map(|(slot, _)| (slot, self.rack_slot_display_name(track, slot)))
            .collect()
    }

    /// Delete every slot of `track` no scene enables, as one undo step.
    /// Never automatic: the panel asks first. Returns the deleted slots'
    /// positions (before the delete).
    pub fn purge_unused_rack_slots_recorded(&mut self, track: usize) -> Result<Vec<usize>, String> {
        let unused: Vec<usize> = self
            .state
            .unused_rack_slots(track)
            .into_iter()
            .map(|(slot, _)| slot)
            .collect();
        if unused.is_empty() {
            return Ok(unused);
        }
        self.apply_recorded_instrument_binding_mutation(
            track,
            "Purge unused rack layers",
            |app| {
                for slot in unused.iter().rev() {
                    app.delete_rack_slot_with_identity(track, *slot)?;
                }
                Ok(())
            },
        )?;
        Ok(unused)
    }

    /// Re-resolve every global macro mapping against the live state and
    /// publish the overrides (after a positional rack slot edit moved
    /// targets, so engaged macros keep driving their device).
    pub(crate) fn revalidate_live_macro_targets(&mut self) {
        let state = std::sync::Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            super::projects::resolve_live_macro_target(
                &state,
                effect_descriptors,
                instrument_descriptors,
                buses,
                scope,
                target,
            )
        });
        self.state
            .publish_macro_overrides(self.macro_engine.override_snapshot());
    }

    /// Delete rack slot `slot` of `track` with everything that names slots by
    /// position: its device identity, the slots after it keeping theirs; the
    /// process bindings and fan-outs of every pattern and the live chain;
    /// project-lane overrides; and global macro mappings (the graph delete
    /// itself moves rack macro targets and slot-FX hosts). Unrecorded: the
    /// caller records an instrument-binding step around it.
    pub fn delete_rack_slot_with_identity(
        &mut self,
        track: usize,
        slot: usize,
    ) -> Result<(), String> {
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.graph_controller().delete_rack_slot(track, slot)?;
        self.device_registry.remove_rack_slot(track_id, slot);
        let shift = crate::process::RackSlotShift::Removed(slot);
        self.state.shift_rack_slot_process_targets(track, shift);
        self.macro_engine.remap_rack_slot_mappings(track, shift);
        self.revalidate_live_macro_targets();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    /// A rack slot's name as its row shows it.
    pub fn rack_slot_display_name(&self, track: usize, slot: usize) -> String {
        let Some(rack_slot) = self
            .state
            .live_rack_track_snapshot(track)
            .and_then(|rack| rack.slots.get(slot).cloned())
        else {
            return String::new();
        };
        let raw = match rack_slot.instrument_type {
            InstrumentType::Sampler => rack_slot
                .sample_id
                .as_ref()
                .map(|(_, name, _)| name.clone())
                .unwrap_or_default(),
            _ => rack_slot
                .track_sound_state
                .loaded_preset
                .clone()
                .unwrap_or_default(),
        };
        super::graph::instrument_display_name(&raw)
    }
}
