//! Project-owned sequencers in an import (docs/rig-spec.md §8, "Sequencers
//! travel with their owner"). A kit's own instances come with the kit
//! (the kit loader); a project-owned one comes only when picked, as a new
//! project-owned instance with a fresh id.
//!
//! Where it plays is decided in ONE place, [`App::place_project_sequencers`]:
//! in the target bank's scenes it gets its source scene's override (routes
//! on the imported tracks, node process slots re-minted) and documents.
//! Every other scene parks it (rig spec §10, `Scene::parked_instances`): it
//! does not run there, so no finished bank changes. Those scenes still get
//! its nodes routed nowhere, so turning it on in another bank later never
//! points it at a master track that merely shares an index.

use std::collections::BTreeMap;

use super::links::{remap_sequencer_routes, Lines};
use super::place::Target;
use super::source::ImportSource;
use super::{App, IdMap};

/// One imported project-owned sequencer.
pub(super) struct ImportedSequencer {
    pub old: u64,
    pub new: u64,
    pub label: String,
    /// The `def-sequencer` name its overrides publish under.
    pub sequencer_name: String,
}

impl App {
    /// A picked project-owned sequencer as a new project-owned instance with
    /// a fresh id. A kind whose package is not attached stays a placeholder
    /// that publishes once it is, as a kit's do.
    pub(super) fn import_project_instance(
        &mut self,
        source: &ImportSource,
        id: u64,
    ) -> Result<ImportedSequencer, String> {
        let record = source
            .file
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| format!("The source has no sequencer {id}"))?;
        self.apply_recorded_instance_mutation("Import sequencer", None, move |app| {
            let new = app.allocate_instance_id();
            let label = Some(record.label.trim().to_string())
                .filter(|label| {
                    !label.is_empty() && !app.instances.list.iter().any(|other| &other.label == label)
                })
                .unwrap_or_else(|| super::super::instances::default_label_in(&app.instances.list, &record.kind));
            app.instances.list.push(crate::project::ProjectInstance {
                id: new,
                kind: record.kind.clone(),
                owner: crate::project::ProjectInstanceOwner::Project,
                label: label.clone(),
            });
            Ok(ImportedSequencer {
                old: record.id,
                new,
                sequencer_name: crate::lisp_host::instance_sequencer_name(
                    crate::lisp_host::kind_name_of(&record.kind),
                    new,
                ),
                label,
            })
        })
    }

    /// Where the imported project-owned sequencers play (see the module
    /// doc), and, when the import made a new bank (`new_bank`), that this
    /// project's own project-owned sequencers do not play there: a new bank
    /// gets §10's default-active rule, so no master conductor runs over the
    /// imported scenes. Runs inside the import's recorded scene-structure
    /// edit.
    pub(super) fn place_project_sequencers(
        &mut self,
        source: &ImportSource,
        sequencers: &[ImportedSequencer],
        ids: &IdMap,
        targets: &[Target],
        new_bank: bool,
        cut: &mut Lines,
    ) {
        use crate::sequencer::ParkedSet;
        if new_bank {
            let masters: Vec<u64> = self
                .instances
                .list
                .iter()
                .filter(|instance| instance.owner.rack().is_none())
                .map(|instance| instance.id)
                .filter(|id| !sequencers.iter().any(|sequencer| sequencer.new == *id))
                .collect();
            self.state.with_scenes_mut(|scenes| {
                for target in targets {
                    for id in &masters {
                        scenes.set_parked_in_scenes(ParkedSet::Instances, *id, true, target.scene..target.scene + 1);
                    }
                }
            });
        }
        if sequencers.is_empty() {
            return;
        }
        let source_of: BTreeMap<usize, usize> = targets
            .iter()
            .map(|target| (target.scene, target.source_scene.unwrap_or_else(|| source.current_scene())))
            .collect();
        self.state.with_scenes_mut(|scenes| {
            // Fresh node process slot ids: one id per (instance, source
            // slot), shared by every scene's copy, never the source's.
            let mut reminter = crate::lisp_host::GraphNodeProcessReminter::new(
                super::super::instances::all_override_entries(scenes),
            );
            for scene in 0..scenes.scenes.len() {
                let active = source_of.get(&scene).copied();
                let pattern = &source.file.patterns[active.unwrap_or_else(|| source.current_scene())];
                for sequencer in sequencers {
                    // On only in the target bank (rig spec §8, §10).
                    scenes.set_parked_in_scenes(ParkedSet::Instances, sequencer.new, active.is_none(), scene..scene + 1);
                    let Some(mut graph) = pattern
                        .graph_overrides
                        .iter()
                        .find(|graph| graph.sequencer_id == sequencer.old)
                        .cloned()
                    else {
                        continue;
                    };
                    graph.sequencer_id = sequencer.new;
                    graph.sequencer_name.clone_from(&sequencer.sequencer_name);
                    graph.owner_rack = None;
                    if active.is_some() {
                        remap_sequencer_routes(&mut graph, source, ids, &sequencer.label, cut);
                    } else {
                        for node in &mut graph.node_intrinsics {
                            node.route = Some(crate::graph::ProjectGraphRouteOverride::None);
                        }
                    }
                    reminter.remint(sequencer.new, &mut graph);
                    let overrides = &mut scenes.scenes[scene].graph_overrides;
                    overrides.retain(|existing| existing.sequencer_id != sequencer.new);
                    overrides.push(graph);
                    if active.is_none() {
                        continue;
                    }
                    for (name, value) in &pattern.scene_slots {
                        if crate::lisp_host::instance_document_slot_owner(name) != Some(sequencer.old) {
                            continue;
                        }
                        let Some((_, field)) = name.rsplit_once('/') else {
                            continue;
                        };
                        let _ = scenes.scenes[scene].scene_slots.set_override(
                            crate::lisp_host::instance_document_slot(sequencer.new, field),
                            Some(value.clone()),
                        );
                    }
                }
            }
        });
    }
}
