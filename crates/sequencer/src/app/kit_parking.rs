//! Kits on/off per bank: the recorded app-level edits (docs/rig-spec.md §6,
//! eseq-ka5t.13). The state lives on the scenes (`Scene::parked_kits`,
//! `sequencer/state/kit_parking.rs`); this is the bank-scoped, undoable
//! command surface the drum rack panel and the mixer drive.

use super::*;
use crate::sequencer::SceneBankId;

impl App {
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
        let (_, range) = self.bank_scene_range(bank)?;
        let changes = self.state.with_project_scenes(|scenes| {
            scenes.kit_parked_would_change(group_id, parked, range.clone())
        });
        if !changes {
            return Ok(false);
        }
        let changed = self.apply_recorded_scene_structure_mutation(
            if parked { "Turn kit off in bank" } else { "Turn kit on in bank" },
            |app| Ok(app.state.set_kit_parked_in_scenes(group_id, parked, range)),
        )?;
        self.state.publish_scheduler_snapshot();
        Ok(changed)
    }

    /// Kit `group_id` over the scenes of `bank` (`None`: the playing
    /// scene's): on, off or mixed. Production reads the group kind's
    /// `parked-banks` / `mixed-banks` instead.
    #[cfg(test)]
    pub fn kit_state_in_bank(
        &self,
        group_id: u64,
        bank: Option<SceneBankId>,
    ) -> Result<crate::sequencer::KitBankState, String> {
        self.kit_group(group_id)?;
        let (_, range) = self.bank_scene_range(bank)?;
        Ok(self
            .state
            .with_project_scenes(|scenes| scenes.kit_state_in_scenes(group_id, range)))
    }

    /// The drum racks every scene of the project switches off: unused
    /// project-wide (rig spec §6), for a purge list. Nothing is deleted.
    #[cfg(test)]
    pub fn unused_kits(&self) -> Vec<u64> {
        let racks: Vec<u64> = self
            .groups
            .iter()
            .filter(|group| group.is_rack())
            .map(|group| group.id)
            .collect();
        self.state
            .with_project_scenes(|scenes| scenes.kits_parked_in_every_scene(&racks))
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
