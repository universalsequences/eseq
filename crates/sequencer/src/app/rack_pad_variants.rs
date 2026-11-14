//! Drum-rack pad variants: recorded edits (docs/rack-pad-variants-spec.md).
//!
//! A variant pad plays its member track with saved instrument values layered
//! over the patch. Every edit here rewrites one rack's pad map through
//! [`App::apply_rack_pads_edit`], so it is one undo entry (a value drag on
//! one param coalesces into one) and republishes the pad runtime.

use super::edit::{EditError, EditOutcome};
use super::history::{MergeKey, RackPadsState};
use super::App;
use crate::project::{ProjectRackConfig, ProjectRackPad};
use crate::sequencer::{DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE, MAX_PAD_VARIANT};

impl App {
    fn drum_rack_config(&self, group_id: u64) -> Result<&ProjectRackConfig, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        group
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))
    }

    /// `group_id`'s pads and choke groups as an undo patch holds them.
    pub fn rack_pads_state(&self, group_id: u64) -> Result<RackPadsState, String> {
        let rack = self.drum_rack_config(group_id)?;
        Ok(RackPadsState {
            pads: rack.pads.clone(),
            choke_groups: rack.choke_groups.clone(),
        })
    }

    /// Replace `group_id`'s pads with `state` (an edit's write and its
    /// undo/redo) and republish the rack runtime. Returns the pads before.
    pub(crate) fn write_rack_pads(
        &mut self,
        group_id: u64,
        state: &RackPadsState,
    ) -> Result<RackPadsState, String> {
        let group = self
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let member_count = group.members.len();
        let rack = group
            .rack
            .as_mut()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        let before = RackPadsState {
            pads: std::mem::replace(&mut rack.pads, state.pads.clone()),
            choke_groups: std::mem::replace(&mut rack.choke_groups, state.choke_groups.clone()),
        };
        rack.sanitize(member_count);
        self.publish_rack_choke_runtime();
        Ok(before)
    }

    fn rack_pad_at_note(&self, group_id: u64, pad_note: i32) -> Result<(usize, &ProjectRackPad), String> {
        let rack = self.drum_rack_config(group_id)?;
        let index = rack
            .pad_index_for_note(pad_note)
            .ok_or_else(|| format!("Drum rack has no pad {pad_note}"))?;
        Ok((index, &rack.pads[index]))
    }

    /// Add a variant pad of member track `track` on the empty note
    /// `pad_note` (spec §6.1, "click an empty cell"). It starts with no
    /// values, so it sounds like the plain pad until a knob is turned.
    /// Returns the new variant id.
    pub fn add_rack_pad_variant(
        &mut self,
        group_id: u64,
        track: usize,
        pad_note: i32,
    ) -> Result<u32, EditError> {
        let fail = EditError::ReplayFailed;
        if !(DRUM_RACK_FIRST_PAD_NOTE..=DRUM_RACK_LAST_PAD_NOTE).contains(&pad_note) {
            return Err(fail(format!("Unsupported drum rack pad note {pad_note}")));
        }
        let group = (self.groups.iter().find(|group| group.id == group_id))
            .ok_or_else(|| fail(format!("Track group {group_id} does not exist")))?;
        let member = (group.members.iter().position(|member| *member == track))
            .ok_or_else(|| fail(format!("Track {track} is not in this drum rack")))?;
        let rack = self.drum_rack_config(group_id).map_err(fail)?;
        if rack.pad_index_for_note(pad_note).is_some() {
            return Err(fail(format!("Pad {pad_note} is already taken")));
        }
        let variant = rack.next_variant_id(member);
        if variant > MAX_PAD_VARIANT {
            return Err(fail("This member has no variant ids left".to_string()));
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        after.pads.push(ProjectRackPad::new_variant(pad_note, member, variant));
        after.choke_groups.push(None);
        let key = MergeKey::new(format!("rack-pad-add:{group_id}:{pad_note}:{variant}"));
        self.apply_rack_pads_edit(group_id, "Add pad variant", key, after)?;
        crate::app::edit::finish_active_gesture(self);
        Ok(variant)
    }

    /// Delete a variant pad. A member's plain pad is not a variant and is
    /// removed with its track instead. Steps that played the variant play
    /// the plain pad (spec §4.3).
    pub fn remove_rack_pad_variant(
        &mut self,
        group_id: u64,
        pad_note: i32,
    ) -> Result<EditOutcome, EditError> {
        let fail = EditError::ReplayFailed;
        let (index, pad) = self.rack_pad_at_note(group_id, pad_note).map_err(fail)?;
        if pad.is_plain() {
            return Err(fail("A member's plain pad is not a variant".to_string()));
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        after.pads.remove(index);
        if index < after.choke_groups.len() {
            after.choke_groups.remove(index);
        }
        let key = MergeKey::new(format!("rack-pad-remove:{group_id}:{pad_note}"));
        let outcome = self.apply_rack_pads_edit(group_id, "Delete pad variant", key, after)?;
        crate::app::edit::finish_active_gesture(self);
        Ok(outcome)
    }

    /// Name a variant pad (`None` or blank: show its note).
    pub fn rename_rack_pad_variant(
        &mut self,
        group_id: u64,
        pad_note: i32,
        name: Option<String>,
    ) -> Result<EditOutcome, EditError> {
        let fail = EditError::ReplayFailed;
        let name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        let (index, pad) = self.rack_pad_at_note(group_id, pad_note).map_err(fail)?;
        if pad.is_plain() {
            return Err(fail("A member's plain pad shows its track name".to_string()));
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        after.pads[index].name = name;
        let key = MergeKey::new(format!("rack-pad-name:{group_id}:{pad_note}"));
        let outcome = self.apply_rack_pads_edit(group_id, "Rename pad variant", key, after)?;
        crate::app::edit::finish_active_gesture(self);
        Ok(outcome)
    }

    /// Set (`Some`) or clear (`None`) one instrument param value of a
    /// variant pad. A drag's writes on one param coalesce into one undo
    /// entry; the gesture closes like any knob drag's.
    pub fn set_rack_pad_variant_value(
        &mut self,
        group_id: u64,
        pad_note: i32,
        param: &str,
        value: Option<f32>,
    ) -> Result<EditOutcome, EditError> {
        let fail = EditError::ReplayFailed;
        if param.trim().is_empty() {
            return Err(fail("A variant value needs a param name".to_string()));
        }
        if value.is_some_and(|value| !value.is_finite()) {
            return Err(fail(format!("Variant value for {param} is not a number")));
        }
        let (index, pad) = self.rack_pad_at_note(group_id, pad_note).map_err(fail)?;
        if pad.is_plain() {
            return Err(fail(
                "A member's plain pad plays the patch; edit the patch instead".to_string(),
            ));
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        let values = &mut after.pads[index].values;
        match value {
            Some(value) => {
                values.insert(param.to_string(), value);
            }
            None => {
                values.remove(param);
            }
        }
        let key = MergeKey::new(format!("rack-pad-value:{group_id}:{pad_note}:{param}"));
        self.apply_rack_pads_edit(group_id, "Set pad variant value", key, after)
    }

    /// Set several of a variant pad's values as one edit (a custom panel's
    /// envelope or latent-map write); a drag's writes on the same params
    /// coalesce into one undo entry.
    pub fn set_rack_pad_variant_values(
        &mut self,
        group_id: u64,
        pad_note: i32,
        updates: &[(String, f32)],
    ) -> Result<EditOutcome, EditError> {
        let fail = EditError::ReplayFailed;
        if updates.is_empty() {
            return Ok(EditOutcome::NoOp);
        }
        if updates
            .iter()
            .any(|(name, value)| name.trim().is_empty() || !value.is_finite())
        {
            return Err(fail("Variant values need param names and numbers".to_string()));
        }
        let (index, pad) = self.rack_pad_at_note(group_id, pad_note).map_err(fail)?;
        if pad.is_plain() {
            return Err(fail(
                "A member's plain pad plays the patch; edit the patch instead".to_string(),
            ));
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        for (name, value) in updates {
            after.pads[index].values.insert(name.clone(), *value);
        }
        let mut names: Vec<&str> = updates.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        let key = MergeKey::new(format!(
            "rack-pad-values:{group_id}:{pad_note}:{}",
            names.join(",")
        ));
        self.apply_rack_pads_edit(group_id, "Set pad variant values", key, after)
    }

    /// Drop every value of a variant pad (it then sounds like the plain pad).
    pub fn clear_rack_pad_variant_values(
        &mut self,
        group_id: u64,
        pad_note: i32,
    ) -> Result<EditOutcome, EditError> {
        let fail = EditError::ReplayFailed;
        let (index, pad) = self.rack_pad_at_note(group_id, pad_note).map_err(fail)?;
        if pad.is_plain() || pad.values.is_empty() {
            return Ok(EditOutcome::NoOp);
        }
        let mut after = self.rack_pads_state(group_id).map_err(fail)?;
        after.pads[index].values.clear();
        let key = MergeKey::new(format!("rack-pad-clear:{group_id}:{pad_note}"));
        let outcome = self.apply_rack_pads_edit(group_id, "Clear pad variant values", key, after)?;
        crate::app::edit::finish_active_gesture(self);
        Ok(outcome)
    }
}
