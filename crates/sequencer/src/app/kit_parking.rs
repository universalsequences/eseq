//! Kits on/off per bank: the recorded app-level edits (docs/rig-spec.md §6,
//! eseq-ka5t.13). The state lives on the scenes (`Scene::parked_kits`,
//! `sequencer/state/kit_parking.rs`); this is the bank-scoped, undoable
//! command surface the drum rack panel and the mixer drive.

use super::*;
use crate::sequencer::{ParkedSet, SceneBankId};

impl App {
    /// Switch `id` off (`parked`) or on in `set` in every scene of `bank`
    /// (the viewed bank; `None`: the playing scene's), as ONE
    /// scene-structure undo step labelled `labels.0` (off) or `labels.1`
    /// (on), then publish. Other banks are untouched. Returns whether
    /// anything changed: a bank already all off/on records nothing. The one
    /// recorded bank toggle kits and project-owned instances share (rig spec
    /// §6, §10); the caller validates `id`.
    pub(super) fn set_parked_in_bank_recorded(
        &mut self,
        set: ParkedSet,
        id: u64,
        parked: bool,
        bank: Option<SceneBankId>,
        labels: (&'static str, &'static str),
    ) -> Result<bool, String> {
        let (_, range) = self.bank_scene_range(bank)?;
        let changes = self
            .state
            .with_project_scenes(|scenes| scenes.parked_would_change(set, id, parked, range.clone()));
        if !changes {
            return Ok(false);
        }
        let changed = self.apply_recorded_scene_structure_mutation(
            if parked { labels.0 } else { labels.1 },
            |app| Ok(app.state.set_parked_in_scenes(set, id, parked, range)),
        )?;
        self.state.publish_scheduler_snapshot();
        Ok(changed)
    }

    /// `id`'s on/off in `set` over the scenes of `bank` (`None`: the
    /// playing scene's). Production reads the host rows' `parked-banks` /
    /// `mixed-banks` instead.
    #[cfg(test)]
    pub fn parked_state_in_bank(
        &self,
        set: ParkedSet,
        id: u64,
        bank: Option<SceneBankId>,
    ) -> Result<crate::sequencer::BankState, String> {
        let (_, range) = self.bank_scene_range(bank)?;
        Ok(self
            .state
            .with_project_scenes(|scenes| scenes.parked_state_in_scenes(set, id, range)))
    }

    /// The drum rack `group_id`, or why it is not one.
    fn kit_group(&self, group_id: u64) -> Result<&crate::project::ProjectTrackGroup, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if !group.is_rack() {
            return Err(format!("Track group {group_id} is not a drum rack"));
        }
        Ok(group)
    }

    /// Switch kit `group_id` off (`parked`) or on in every scene of `bank`
    /// (the viewed bank; `None`: the playing scene's), as ONE scene-structure
    /// undo step. Clip pointers are never touched, so off-then-on restores
    /// every scene exactly; other banks are untouched. Returns whether
    /// anything changed (a bank already all off/on records nothing).
    pub fn set_kit_parked_in_bank_recorded(
        &mut self,
        group_id: u64,
        parked: bool,
        bank: Option<SceneBankId>,
    ) -> Result<bool, String> {
        self.kit_group(group_id)?;
        self.set_parked_in_bank_recorded(
            ParkedSet::Kits,
            group_id,
            parked,
            bank,
            ("Turn kit off in bank", "Turn kit on in bank"),
        )
    }

    /// Kit `group_id` over the scenes of `bank` (`None`: the playing
    /// scene's): on, off or mixed. Production reads the group kind's
    /// `parked-banks` / `mixed-banks` instead.
    #[cfg(test)]
    pub fn kit_state_in_bank(
        &self,
        group_id: u64,
        bank: Option<SceneBankId>,
    ) -> Result<crate::sequencer::BankState, String> {
        self.kit_group(group_id)?;
        self.parked_state_in_bank(ParkedSet::Kits, group_id, bank)
    }

    /// The drum racks every scene of the project switches off: unused
    /// project-wide (rig spec §6), for a purge list. Nothing is deleted.
    #[cfg(test)]
    pub fn unused_kits(&self) -> Vec<u64> {
        self.state.with_project_scenes(|scenes| {
            self.groups
                .iter()
                .filter(|group| group.is_rack() && scenes.kit_parked_in_every_scene(group.id))
                .map(|group| group.id)
                .collect()
        })
    }

    /// The position of the bank holding the playing scene, when the playing
    /// scene switches kit `group_id` off: a clip launch on it is refused
    /// (it would change the scene's clip and stay silent).
    pub fn kit_parked_bank_index(&self, group_id: u64) -> Option<usize> {
        self.state.with_project_scenes(|scenes| {
            if !scenes.kit_parked_in_scene(scenes.current_scene, group_id) {
                return None;
            }
            let bank = scenes.scene_bank_containing(scenes.current_scene)?;
            scenes.scene_banks().iter().position(|b| b.id == bank)
        })
    }
}
