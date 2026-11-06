//! Track display order (eseq-ssd6): the order the step grid, the mixer and
//! the arrangement draw track rows in, and the recorded edits that move a
//! track or a group block within it.
//!
//! Track indices are the runtime and authoring address of a track
//! everywhere (scenes, lanes, routes, scripts), so a reorder never moves
//! them. `App::track_order` holds stable ids instead, and every view lays out
//! by [`App::display_track_order`]. That reader heals the stored list rather
//! than trusting it: ids of deleted tracks are skipped, tracks the list does
//! not name yet (a new or duplicated track) are placed by their index, and
//! the result is put in structural form, so a group's tracks always form one
//! contiguous block exactly as the Lisp views draw it
//! (`content/ui/drum-rack-v2.lisp`, `group-track-order`).

use super::App;
use crate::sequencer::TrackId;

/// Nesting bound for walking racks: one level deep by construction, so this
/// only guards a malformed cyclic file.
const MAX_RACK_DEPTH: usize = 8;

/// Each track's display position, indexed by track, from `order` (display
/// order); a track `order` does not list keeps its index.
pub fn display_rank(order: &[usize], count: usize) -> Vec<usize> {
    let mut rank: Vec<usize> = (0..count).collect();
    for (position, &track) in order.iter().enumerate() {
        if track < count {
            rank[track] = position;
        }
    }
    rank
}

impl App {
    /// Track indices in display order: every track exactly once.
    pub fn display_track_order(&self) -> Vec<usize> {
        let count = self.tracks.len().min(self.track_registry.len());
        if self.track_order.is_empty() {
            // Never reordered: the order the views drew before display order
            // existed — tracks by index, a group at its lowest member, its
            // members in membership order (a rack's: pad order).
            let rank: Vec<usize> = (0..count).collect();
            return self.structural_track_order(count, &rank, false);
        }
        let rank = display_rank(&self.healed_track_order(count), count);
        self.structural_track_order(count, &rank, true)
    }

    /// Track `track`'s number as the user sees it: its display position from
    /// 1 (the mute button's number, route menus, labels). Out of range: the
    /// index from 1. Computes the order: for many labels at once, number
    /// from one `display_track_order` instead.
    pub fn track_number(&self, track: usize) -> usize {
        self.display_track_order()
            .iter()
            .position(|&t| t == track)
            .map_or(track + 1, |position| position + 1)
    }

    /// Every track's number (`track_number`), indexed by track: for labels
    /// built in a loop.
    pub fn track_numbers(&self) -> Vec<usize> {
        display_rank(&self.display_track_order(), self.tracks.len())
            .into_iter()
            .map(|position| position + 1)
            .collect()
    }

    /// The display order as stable ids, for the project file: empty while the
    /// project was never reordered (the legacy order needs no storage).
    pub(crate) fn saved_track_order(&self) -> Vec<TrackId> {
        if self.track_order.is_empty() {
            return Vec::new();
        }
        self.display_track_ids()
    }

    fn display_track_ids(&self) -> Vec<TrackId> {
        self.display_track_order()
            .into_iter()
            .filter_map(|track| self.track_registry.id_at(track))
            .collect()
    }

    /// The stored order as indices: known ids once each, then each track the
    /// list does not name, ascending — a trailing run of them (appended
    /// tracks) at the end, any other right after its index predecessor (a
    /// duplicate lands beside its source).
    fn healed_track_order(&self, count: usize) -> Vec<usize> {
        let mut seen = vec![false; count];
        let mut order = Vec::with_capacity(count);
        for id in &self.track_order {
            let Some(track) = self.track_registry.index_of(*id) else { continue };
            if track < count && !seen[track] {
                seen[track] = true;
                order.push(track);
            }
        }
        let trailing_from = (0..count).rev()
            .take_while(|track| !seen[*track])
            .last()
            .unwrap_or(count);
        for track in 0..trailing_from {
            if seen[track] {
                continue;
            }
            let at = if track == 0 {
                0
            } else {
                order.iter().position(|&other| other == track - 1).map_or(order.len(), |p| p + 1)
            };
            order.insert(at, track);
            seen[track] = true;
        }
        order.extend(trailing_from..count);
        order
    }

    /// The tracks laid out as the views draw them: loose tracks and top-level
    /// groups by `rank` (a group by its lowest-ranked track), each group as
    /// its members then its child racks. `members_by_rank` sorts members by
    /// rank; without it they keep membership order.
    fn structural_track_order(
        &self,
        count: usize,
        rank: &[usize],
        members_by_rank: bool,
    ) -> Vec<usize> {
        let mut grouped = vec![false; count];
        let mut items: Vec<(usize, Vec<usize>)> = Vec::new();
        for group in &self.groups {
            for &member in &group.members {
                if member < count {
                    grouped[member] = true;
                }
            }
            if self.rack_parent_group(group.id).is_some() {
                continue;
            }
            let tracks = self.group_block(group.id, count, rank, members_by_rank, 0);
            if let Some(anchor) = tracks.iter().map(|&track| rank[track]).min() {
                items.push((anchor, tracks));
            }
        }
        for track in 0..count {
            if !grouped[track] {
                items.push((rank[track], vec![track]));
            }
        }
        items.sort_by_key(|(anchor, _)| *anchor);
        let mut order: Vec<usize> = items.into_iter().flat_map(|(_, tracks)| tracks).collect();
        // A track listed by two groups (a malformed file) would appear
        // twice; keep its first place.
        let mut placed = vec![false; count];
        order.retain(|&track| !std::mem::replace(&mut placed[track], true));
        order
    }

    /// Group `group_id`'s tracks in drawn order: its members, then each child
    /// rack's block.
    fn group_block(
        &self,
        group_id: u64,
        count: usize,
        rank: &[usize],
        members_by_rank: bool,
        depth: usize,
    ) -> Vec<usize> {
        let Some(group) = self.groups.iter().find(|group| group.id == group_id) else {
            return Vec::new();
        };
        let mut tracks: Vec<usize> =
            group.members.iter().copied().filter(|&member| member < count).collect();
        if members_by_rank {
            tracks.sort_by_key(|&track| rank[track]);
        }
        if depth < MAX_RACK_DEPTH {
            for &rack in &group.rack_members {
                tracks.extend(self.group_block(rack, count, rank, members_by_rank, depth + 1));
            }
        }
        tracks
    }

    /// The id of the group that lists `track` as a member.
    fn track_holder(&self, track: usize) -> Option<u64> {
        self.groups.iter()
            .find(|group| group.members.contains(&track))
            .map(|group| group.id)
    }

    /// The outermost group enclosing group `group_id` (itself when top-level).
    fn top_level_group(&self, group_id: u64) -> u64 {
        let mut current = group_id;
        for _ in 0..MAX_RACK_DEPTH {
            match self.rack_parent_group(current) {
                Some(parent) => current = self.groups[parent].id,
                None => break,
            }
        }
        current
    }

    /// The tracks of the top-level row `track` belongs to: its top-level
    /// group's block, or just itself when loose.
    fn top_level_block(&self, track: usize) -> Vec<usize> {
        let count = self.tracks.len().min(self.track_registry.len());
        match self.track_holder(track) {
            Some(group) => {
                let top = self.top_level_group(group);
                let order = self.display_track_order();
                let rank = display_rank(&order, count);
                self.group_block(top, count, &rank, true, 0)
            }
            None => vec![track],
        }
    }

    /// Stores `order` (indices) as the display order, then settles it into
    /// structural form so the stored list always reads as drawn.
    fn store_track_order(&mut self, order: &[usize]) {
        self.track_order = order.iter()
            .filter_map(|&track| self.track_registry.id_at(track))
            .collect();
        self.track_order = self.display_track_ids();
    }

    /// Moves `track` to the place of `target` in the display order, joining
    /// `target`'s group (or leaving its own for the top level when `target`
    /// is loose) — dropping a row onto another row, in the step grid, the
    /// mixer or the arrangement. Moving down lands after `target`, moving up
    /// before it, like the scene pills. One undo step.
    pub fn move_track_recorded(&mut self, track: usize, target: usize) -> Result<(), String> {
        let count = self.tracks.len();
        if track >= count || target >= count {
            return Err("Track is out of range".to_string());
        }
        if track == target {
            return Ok(());
        }
        self.apply_recorded_bus_group_structure_mutation("Move track", |app| {
            let before = app.display_track_order();
            let from = before.iter().position(|&t| t == track)
                .ok_or_else(|| "Track is missing from the display order".to_string())?;
            let destination = app.track_holder(target);
            app.regroup_track_unrecorded(track, destination)?;
            let mut order = before;
            order.remove(from);
            let at = order.iter().position(|&t| t == target)
                .ok_or_else(|| "Target track is missing from the display order".to_string())?;
            let at = if from <= at { at + 1 } else { at };
            order.insert(at, track);
            app.store_track_order(&order);
            Ok(())
        })
    }

    /// Moves top-level group `group_id`'s whole block to the place of the
    /// top-level row holding `target` (a loose track or another group's
    /// block): after it moving down, before it moving up. Membership does
    /// not change. One undo step.
    pub fn move_group_recorded(&mut self, group_id: u64, target: usize) -> Result<(), String> {
        if target >= self.tracks.len() {
            return Err("Track is out of range".to_string());
        }
        if !self.groups.iter().any(|group| group.id == group_id) {
            return Err(format!("Track group {group_id} does not exist"));
        }
        if self.top_level_group(group_id) != group_id {
            return Err("A nested rack moves with the group around it".to_string());
        }
        let count = self.tracks.len().min(self.track_registry.len());
        let order = self.display_track_order();
        let rank = display_rank(&order, count);
        let block = self.group_block(group_id, count, &rank, true, 0);
        if block.is_empty() {
            return Err("The group has no tracks to move".to_string());
        }
        let target_block = self.top_level_block(target);
        if target_block.iter().any(|t| block.contains(t)) {
            return Ok(());
        }
        self.apply_recorded_bus_group_structure_mutation("Move group", |app| {
            let moving_down = rank[block[0]] < rank[target_block[0]];
            let mut order: Vec<usize> =
                order.iter().copied().filter(|t| !block.contains(t)).collect();
            let at = if moving_down {
                let last = *target_block.iter().max_by_key(|&&t| rank[t]).unwrap();
                order.iter().position(|&t| t == last).unwrap() + 1
            } else {
                let first = *target_block.iter().min_by_key(|&&t| rank[t]).unwrap();
                order.iter().position(|&t| t == first).unwrap()
            };
            order.splice(at..at, block.iter().copied());
            app.store_track_order(&order);
            Ok(())
        })
    }

    /// Puts `track` at the end of group `group_id`'s block in the display
    /// order: a track dropped onto a group header joins it last.
    pub(super) fn place_track_last_in_group(&mut self, track: usize, group_id: u64) {
        let count = self.tracks.len().min(self.track_registry.len());
        let mut order = self.display_track_order();
        let rank = display_rank(&order, count);
        let Some(last) = self.group_block(group_id, count, &rank, true, 0)
            .into_iter()
            .filter(|&t| t != track)
            .max_by_key(|&t| rank[t])
        else {
            return;
        };
        order.retain(|&t| t != track);
        let at = order.iter().position(|&t| t == last).map_or(order.len(), |p| p + 1);
        order.insert(at, track);
        self.store_track_order(&order);
    }
}

