//! Track routes inside instance documents (a kind's `:routes`): shifting
//! them when a track or rack member is removed, wherever the document is
//! stored — every scene's slots and every rack clip's — and putting the
//! values back on undo.

use crate::process::ProcessLiteral;

use super::{ProjectScenes, RackClipId, SceneSlotStore};

/// Where one instance-document slot value lives.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceSlotLocation {
    Scene(usize),
    RackClip { group_id: u64, clip: RackClipId },
}

/// One slot an edit changed, with its value before the edit.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceSlotChange {
    pub location: InstanceSlotLocation,
    pub name: String,
    pub before: ProcessLiteral,
}

impl InstanceSlotChange {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.name.capacity() + self.before.retained_bytes()
    }
}

impl ProjectScenes {
    /// Run `edit` over every instance-document slot value (`%instance/…`) in
    /// every scene and rack clip; a value it changes (returns true) is
    /// written back. Returns the changed slots with their old values.
    pub fn edit_instance_document_slots(
        &mut self,
        mut edit: impl FnMut(&str, &mut ProcessLiteral) -> bool,
    ) -> Result<Vec<InstanceSlotChange>, String> {
        let mut changes = Vec::new();
        for (index, scene) in self.scenes.iter_mut().enumerate() {
            edit_store(
                &mut scene.scene_slots,
                InstanceSlotLocation::Scene(index),
                &mut edit,
                &mut changes,
            )?;
        }
        for bank in &mut self.rack_banks {
            for clip in &mut bank.clips {
                let location =
                    InstanceSlotLocation::RackClip { group_id: bank.group_id, clip: clip.id };
                edit_store(&mut clip.scene_slots, location, &mut edit, &mut changes)?;
            }
        }
        Ok(changes)
    }

    /// Write `changes`' old values back (undoing `edit_instance_document_slots`).
    /// A location that no longer exists is skipped.
    pub fn restore_instance_document_slots(
        &mut self,
        changes: &[InstanceSlotChange],
    ) -> Result<(), String> {
        for change in changes {
            let store = match change.location {
                InstanceSlotLocation::Scene(index) => {
                    self.scenes.get_mut(index).map(|scene| &mut scene.scene_slots)
                }
                InstanceSlotLocation::RackClip { group_id, clip } => self
                    .rack_banks
                    .iter_mut()
                    .find(|bank| bank.group_id == group_id)
                    .and_then(|bank| bank.clips.iter_mut().find(|c| c.id == clip))
                    .map(|clip| &mut clip.scene_slots),
            };
            if let Some(store) = store {
                store.write_literal(change.name.clone(), change.before.clone())?;
            }
        }
        Ok(())
    }
}

fn edit_store(
    store: &mut SceneSlotStore,
    location: InstanceSlotLocation,
    edit: &mut impl FnMut(&str, &mut ProcessLiteral) -> bool,
    changes: &mut Vec<InstanceSlotChange>,
) -> Result<(), String> {
    let slots: Vec<(String, ProcessLiteral)> = store
        .values()
        .iter()
        .filter(|(name, _)| crate::lisp_host::instance_document_slot_owner(name).is_some())
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    for (name, before) in slots {
        let mut value = before.clone();
        if edit(&name, &mut value) && value != before {
            store.write_literal(name.clone(), value)?;
            changes.push(InstanceSlotChange { location: location.clone(), name, before });
        }
    }
    Ok(())
}

/// Shift the routes at `path` (below the document field: map keys and `*`
/// for every list item) inside `value` for the removal of route `removed`:
/// that route becomes -1 (none), later ones move down by one, negative ones
/// (none, or a kind's own markers) stay. Returns whether anything changed.
pub fn remap_document_routes(value: &mut ProcessLiteral, path: &[String], removed: usize) -> bool {
    match path.split_first() {
        None => match value {
            ProcessLiteral::Number(route) if *route >= 0.0 && route.fract() == 0.0 => {
                let before = *route as usize;
                let after = super::remap_optional_track_after_delete(before, removed);
                *route = after.map_or(-1.0, |after| after as f64);
                after != Some(before)
            }
            _ => false,
        },
        Some((step, rest)) if step == "*" => match value {
            ProcessLiteral::List(items) => {
                let mut changed = false;
                for item in items {
                    changed |= remap_document_routes(item, rest, removed);
                }
                changed
            }
            _ => false,
        },
        Some((step, rest)) => match value {
            ProcessLiteral::Map(map) => {
                let bare = step.trim_start_matches(':');
                let colon = format!(":{bare}");
                match map.get_mut(bare) {
                    Some(inner) => remap_document_routes(inner, rest, removed),
                    None => map
                        .get_mut(&colon)
                        .is_some_and(|inner| remap_document_routes(inner, rest, removed)),
                }
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn row(route: f64) -> ProcessLiteral {
        let mut map = BTreeMap::new();
        map.insert("route".to_string(), ProcessLiteral::Number(route));
        map.insert("mods".to_string(), ProcessLiteral::List(Vec::new()));
        ProcessLiteral::Map(map)
    }

    fn routes(value: &ProcessLiteral) -> Vec<f64> {
        let ProcessLiteral::List(rows) = value else { panic!("rows") };
        rows.iter()
            .map(|row| match row {
                ProcessLiteral::Map(map) => match map.get("route") {
                    Some(ProcessLiteral::Number(n)) => *n,
                    _ => panic!("route"),
                },
                _ => panic!("row"),
            })
            .collect()
    }

    #[test]
    fn removing_a_route_turns_its_rows_off_and_shifts_later_ones() {
        let mut rows = ProcessLiteral::List(vec![row(0.0), row(1.0), row(2.0), row(3.0), row(-1.0), row(-2.0)]);
        let path = vec!["*".to_string(), "route".to_string()];
        assert!(remap_document_routes(&mut rows, &path, 1));
        assert_eq!(routes(&rows), vec![0.0, -1.0, 1.0, 2.0, -1.0, -2.0]);
        // Removing past every route changes nothing.
        assert!(!remap_document_routes(&mut rows, &path, 9));
    }

    #[test]
    fn nested_paths_reach_every_patterns_rows() {
        let mut pattern = BTreeMap::new();
        pattern.insert("rows".to_string(), ProcessLiteral::List(vec![row(2.0), row(0.0)]));
        let mut patterns = ProcessLiteral::List(vec![ProcessLiteral::Map(pattern)]);
        let path: Vec<String> = ["*", "rows", "*", "route"].iter().map(|s| s.to_string()).collect();
        assert!(remap_document_routes(&mut patterns, &path, 0));
        let ProcessLiteral::List(items) = &patterns else { panic!() };
        let ProcessLiteral::Map(map) = &items[0] else { panic!() };
        assert_eq!(routes(&map["rows"]), vec![1.0, -1.0]);
    }
}
