//! Project-owned sequencers on/off per bank: the recorded app-level edits
//! (docs/rig-spec.md §10, eseq-ka5t.1/.6). The state lives on the scenes
//! (`Scene::parked_instances`, `sequencer/state/instance_parking.rs`); this
//! is the bank-scoped, undoable command surface the instance views and the
//! Packages tab drive, modelled on kit parking (`super::kit_parking`).
//!
//! What a newly activated bank can copy: a project-owned instance's
//! settings are per scene (its `:document` slots in each scene's slot store,
//! docs/jaki-kind-spec.md §3, and its graph overrides in each scene's list),
//! so "turn on in bank C with scene X's settings" copies exactly those two
//! from scene X into every scene of bank C, in the same undo step.

use super::*;
use crate::project::ProjectInstance;
use crate::sequencer::{ParkedSet, SceneBankId};

impl App {
    /// Project-owned instance `id`, or why it cannot be switched per bank (a
    /// rack-owned one follows its kit's on/off, §6).
    fn project_owned_instance(&self, id: u64) -> Result<&ProjectInstance, String> {
        let instance = self
            .instances
            .get(id)
            .ok_or_else(|| format!("Instance {id} does not exist"))?;
        if instance.owner.rack().is_some() {
            return Err(format!(
                "{} belongs to a kit; it turns on and off with the kit",
                instance.label
            ));
        }
        Ok(instance)
    }

    /// Switch project-owned instance `id` off (`parked`) or on in every
    /// scene of `bank` (the viewed bank; `None`: the playing scene's), as ONE
    /// scene-structure undo step (the toggle kits share,
    /// `set_parked_in_bank_recorded`). Returns whether anything changed.
    pub fn set_instance_parked_in_bank_recorded(
        &mut self,
        id: u64,
        parked: bool,
        bank: Option<SceneBankId>,
    ) -> Result<bool, String> {
        self.project_owned_instance(id)?;
        self.set_parked_in_bank_recorded(
            ParkedSet::Instances,
            id,
            parked,
            bank,
            ("Turn sequencer off in bank", "Turn sequencer on in bank"),
        )
    }

    /// Turn project-owned instance `id` on in every scene of `bank` with the
    /// settings it has in scene `from_scene` (rev-1 R3 "recall a scene's
    /// settings"): that scene's `:document` slots and graph overrides of the
    /// instance replace the bank's, in the same undo step as the switch.
    /// `from_scene` may be in any bank. Records and publishes only when a
    /// scene of the bank changes; returns whether one did.
    pub fn activate_instance_in_bank_from_scene_recorded(
        &mut self,
        id: u64,
        bank: Option<SceneBankId>,
        from_scene: usize,
    ) -> Result<bool, String> {
        self.project_owned_instance(id)?;
        let (_, range) = self.bank_scene_range(bank)?;
        let (documents, overrides) = self.state.with_project_scenes(|scenes| {
            let scene = scenes
                .scenes
                .get(from_scene)
                .ok_or_else(|| format!("Scene {} does not exist", from_scene + 1))?;
            Ok::<_, String>((
                super::instances::owned_document_slots(&scene.scene_slots, &[id]),
                instance_overrides(&scene.graph_overrides, id),
            ))
        })?;
        // A scene changes when it parks the instance or holds other settings.
        let differs = |scene: &crate::sequencer::Scene| {
            scene.parked_instances.contains(&id)
                || super::instances::owned_document_slots(&scene.scene_slots, &[id]) != documents
                || instance_overrides(&scene.graph_overrides, id) != overrides
        };
        let changes = self
            .state
            .with_project_scenes(|scenes| scenes.clamped_scenes(range.clone()).iter().any(differs));
        if !changes {
            return Ok(false);
        }
        self.apply_recorded_scene_structure_mutation("Turn sequencer on in bank", |app| {
            app.state.with_scenes_mut(|scenes| {
                scenes.set_parked_in_scenes(ParkedSet::Instances, id, false, range.clone());
                let range = scenes.clamped_range(range);
                for scene in &mut scenes.scenes[range] {
                    let owned = super::instances::owned_document_slots(&scene.scene_slots, &[id]);
                    super::instances::replace_document_slots(&mut scene.scene_slots, owned, documents.clone());
                    if instance_overrides(&scene.graph_overrides, id) != overrides {
                        scene.graph_overrides.retain(|graph| graph.sequencer_id != id);
                        scene.graph_overrides.extend(overrides.iter().cloned());
                    }
                }
            });
            Ok(())
        })?;
        self.state.publish_scheduler_snapshot();
        Ok(true)
    }

    /// The scene of bank `bank` whose settings "turn on like bank A" copies:
    /// the first scene there that plays instance `id`, else its first scene.
    pub fn instance_settings_scene_in_bank(&self, id: u64, bank: SceneBankId) -> Result<usize, String> {
        let (_, range) = self.bank_scene_range(Some(bank))?;
        self.state.with_project_scenes(|scenes| {
            range
                .clone()
                .find(|scene| !scenes.instance_parked_in_scene(*scene, id))
                .or_else(|| (!range.is_empty()).then_some(range.start))
                .ok_or_else(|| "That bank has no scenes".to_string())
        })
    }

    /// Instance `id` over the scenes of `bank` (`None`: the playing
    /// scene's). Production reads `project.instances`' `:parked-banks` /
    /// `:mixed-banks` instead.
    #[cfg(test)]
    pub fn instance_state_in_bank(
        &self,
        id: u64,
        bank: Option<SceneBankId>,
    ) -> Result<crate::sequencer::BankState, String> {
        self.parked_state_in_bank(ParkedSet::Instances, id, bank)
    }

    /// The project-owned instances every scene switches off: unused
    /// project-wide (§10), listed for a purge, never deleted for you.
    pub fn unused_instances(&self) -> Vec<u64> {
        self.state.with_project_scenes(|scenes| {
            self.instances
                .list
                .iter()
                .filter(|instance| instance.owner.rack().is_none())
                .map(|instance| instance.id)
                .filter(|id| scenes.parked_in_every_scene(ParkedSet::Instances, *id))
                .collect()
        })
    }

    /// Delete every unused project-owned instance as ONE undo step (the
    /// explicit purge, §10): the ordinary instance delete, overrides,
    /// documents, owned processes and parked entries included.
    pub fn purge_unused_instances_recorded(&mut self) -> Result<Vec<ProjectInstance>, String> {
        let unused = self.unused_instances();
        self.delete_instances_recorded(&unused, "Purge unused sequencers", None)
    }

    /// Park project-owned instance `id` in every scene outside `bank`
    /// (`None`: the playing scene's): §10's default-active rule for a new
    /// instance (a create, a move back to the project). Called inside the
    /// recording instance edit, whose history memento carries every scene's
    /// parked set.
    pub(super) fn park_new_instance_outside_bank(
        &mut self,
        id: u64,
        bank: Option<SceneBankId>,
    ) -> Result<(), String> {
        // A bank that does not resolve (a stale :bank-id) falls back to the
        // playing scene's; a project with no bank parks nothing.
        let Ok((_, range)) = self
            .bank_scene_range(bank)
            .or_else(|_| self.bank_scene_range(None))
        else {
            return Ok(());
        };
        self.state
            .with_scenes_mut(|scenes| scenes.park_outside(ParkedSet::Instances, id, range));
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    /// Forget instances `ids` in every scene's parked set (deleted, or now
    /// owned by a kit).
    pub(super) fn forget_parked_instances(&self, ids: &[u64]) {
        if self
            .state
            .with_scenes_mut(|scenes| scenes.forget_parked(ParkedSet::Instances, ids))
        {
            self.state.publish_scheduler_snapshot();
        }
    }
}

/// The graph overrides of instance `id` in one list.
fn instance_overrides(
    graphs: &[crate::graph::ProjectGraphOverrides],
    id: u64,
) -> Vec<crate::graph::ProjectGraphOverrides> {
    graphs.iter().filter(|graph| graph.sequencer_id == id).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::history::HistoryReplay;
    use crate::app::test_rig::{launch, new_scene_in, two_banks, Rig};
    use crate::project::ProjectInstanceOwner;
    use crate::sequencer::{BankState, SceneSwapInheritance};
    use eseqlisp::runtime::Runtime;

    const KIND: &str = "scratch:cond";

    /// A registered graph kind `cond` (kind id `scratch:cond`); keep the
    /// runtime alive for the test.
    fn conductor_kind(app: &App) -> Runtime {
        crate::lisp_host::clear_kind_registry();
        let mut runtime = Runtime::new();
        crate::lisp_host::register_graph_authoring_natives(&mut runtime, std::sync::Arc::clone(&app.state));
        runtime
            .eval_str(
                "(def-kind cond
                   :sequencer (:shape (line 2) :max-poly 1
                               (def-node nrn :route 0
                                 :params ((threshold :float 0 4 :default 0.5)))
                               (edges :from nrn :to nrn :topology (all-to-all)
                                 :params ((weight :float -1 1 :default 0))))
                   :document ((amount 1))
                   :view (lambda (self) nil))",
            )
            .expect("def-kind evaluates");
        assert!(crate::lisp_host::registered_kind(KIND).is_some());
        runtime
    }

    fn parked_scenes(app: &App, id: u64) -> Vec<bool> {
        app.state.with_project_scenes(|scenes| {
            (0..scenes.scenes.len())
                .map(|scene| scenes.instance_parked_in_scene(scene, id))
                .collect()
        })
    }

    fn applied<E: std::fmt::Debug>(replay: HistoryReplay<E>) {
        assert!(matches!(replay, HistoryReplay::Applied(_)), "{replay:?}");
    }

    fn create_in(app: &mut App, bank: Option<SceneBankId>) -> u64 {
        app.create_instance_in_bank_recorded(KIND, ProjectInstanceOwner::Project, None, bank)
            .expect("create")
    }

    /// Whether each scene's launch snapshot parks instance `id`.
    fn parked_when_launched(app: &mut App, id: u64) -> Vec<bool> {
        (0..app.state.scene_count())
            .map(|scene| {
                launch(app, scene);
                app.state.latest_scheduler_snapshot().instance_parked(id)
            })
            .collect()
    }

    #[test]
    fn a_new_project_instance_is_active_only_in_the_bank_it_is_made_in() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_b));
        assert_eq!(parked_scenes(app, id), vec![true, true, false, false]);
        assert_eq!(app.instance_state_in_bank(id, Some(bank_b)).unwrap(), BankState::On);
        // One undo takes the instance and its parking away; redo brings both.
        applied(crate::app::edit::undo(app));
        assert!(app.instances.get(id).is_none());
        assert_eq!(parked_scenes(app, id), vec![false; 4]);
        applied(crate::app::edit::redo(app));
        assert_eq!(parked_scenes(app, id), vec![true, true, false, false]);
        // The playing scene's bank (A) without a :bank-id.
        let in_a = create_in(app, None);
        assert_eq!(parked_scenes(app, in_a), vec![false, false, true, true]);
    }

    #[test]
    fn a_parked_instance_is_gated_in_bank_a_and_runs_in_bank_b() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (bank_a, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_b));
        assert_eq!(parked_when_launched(app, id), vec![true, true, false, false]);
        assert_eq!(app.instance_state_in_bank(id, Some(bank_a)).unwrap(), BankState::Off);
        assert!(
            !app.set_instance_parked_in_bank_recorded(id, true, Some(bank_a)).unwrap(),
            "already off records nothing"
        );
        // On in bank A, one undo step.
        assert!(app.set_instance_parked_in_bank_recorded(id, false, Some(bank_a)).unwrap());
        assert_eq!(parked_when_launched(app, id), vec![false; 4]);
        applied(crate::app::edit::undo(app));
        assert_eq!(parked_when_launched(app, id), vec![true, true, false, false]);
        // A mixed bank: one scene of bank B off.
        app.state.with_scenes_mut(|scenes| scenes.set_parked_in_scenes(crate::sequencer::ParkedSet::Instances, id, true, 2..3));
        assert_eq!(app.instance_state_in_bank(id, Some(bank_b)).unwrap(), BankState::Mixed);
    }

    #[test]
    fn new_scenes_inherit_parked_instances() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (bank_a, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_b));
        launch(app, 0);
        // A clone of a scene that parks it, and a `+` scene in bank A.
        let cloned = new_scene_in(app, bank_a, SceneSwapInheritance::Source(0));
        assert!(app.state.with_project_scenes(|s| s.instance_parked_in_scene(cloned, id)));
        let blank = new_scene_in(app, bank_b, SceneSwapInheritance::BankNeighbour);
        assert!(
            !app.state.with_project_scenes(|s| s.instance_parked_in_scene(blank, id)),
            "a + scene in bank B plays like bank B"
        );
    }

    #[test]
    fn parked_instances_round_trip_a_v20_file_and_legacy_files_park_nothing() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_b));
        let saved = app.capture_project("instance-parking").unwrap();
        assert_eq!(saved.version, 20);
        assert_eq!(saved.scene_parked_instances, vec![vec![id], vec![id], vec![], vec![]]);
        let mut legacy = saved.clone();
        legacy.scene_parked_instances.clear();
        legacy.version = 19;
        rig.reload("instance-parking", saved);
        assert_eq!(parked_scenes(rig.app(), id), vec![true, true, false, false]);
        rig.reload("legacy", legacy);
        assert_eq!(parked_scenes(rig.app(), id), vec![false; 4], "a v19 file plays it everywhere");
    }

    #[test]
    fn deleting_an_instance_forgets_its_parking_and_undo_restores_it() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_b));
        app.delete_instance_recorded(id).unwrap();
        assert_eq!(parked_scenes(app, id), vec![false; 4]);
        applied(crate::app::edit::undo(app));
        assert_eq!(parked_scenes(app, id), vec![true, true, false, false]);
        // A duplicate plays where its source plays.
        let copy = app.duplicate_instance_recorded(id).unwrap();
        assert_eq!(parked_scenes(app, copy), vec![true, true, false, false]);
    }

    #[test]
    fn unused_instances_are_listed_and_purged_in_one_undo_step() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (bank_a, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let used = create_in(app, Some(bank_a));
        let unused = create_in(app, Some(bank_b));
        assert!(app.unused_instances().is_empty());
        app.set_instance_parked_in_bank_recorded(unused, true, Some(bank_b)).unwrap();
        assert_eq!(app.unused_instances(), vec![unused]);
        let removed = app.purge_unused_instances_recorded().unwrap();
        assert_eq!(removed.iter().map(|i| i.id).collect::<Vec<_>>(), vec![unused]);
        assert!(app.instances.get(used).is_some());
        applied(crate::app::edit::undo(app));
        assert!(app.instances.get(unused).is_some(), "one undo brings it back");
        assert_eq!(app.unused_instances(), vec![unused]);
    }

    #[test]
    fn turning_on_in_a_bank_can_copy_a_scenes_settings() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (bank_a, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_a));
        let slot = crate::lisp_host::instance_document_slot(id, "amount");
        app.state.with_scenes_mut(|scenes| {
            scenes.scenes[1]
                .scene_slots
                .set_override(slot.clone(), Some(crate::process::ProcessLiteral::Number(7.0)))
                .unwrap();
        });
        assert_eq!(app.instance_settings_scene_in_bank(id, bank_a).unwrap(), 0);
        app.activate_instance_in_bank_from_scene_recorded(id, Some(bank_b), 1).unwrap();
        let amounts: Vec<Option<crate::process::ProcessLiteral>> = app.state.with_project_scenes(|s| {
            (0..4).map(|scene| s.scenes[scene].scene_slots.get(&slot).cloned()).collect()
        });
        let seven = Some(crate::process::ProcessLiteral::Number(7.0));
        assert_eq!(amounts, vec![None, seven.clone(), seven.clone(), seven]);
        assert_eq!(parked_scenes(app, id), vec![false; 4]);
        // One undo: parked again in bank B, and bank B's document as before.
        applied(crate::app::edit::undo(app));
        assert_eq!(parked_scenes(app, id), vec![false, false, true, true]);
        assert!(app.state.with_project_scenes(|s| s.scenes[2].scene_slots.get(&slot).is_none()));
    }

    /// A track process slot owned by `owner`, on track 0's live chain and
    /// saved into its pattern.
    fn owned_slot(app: &mut App, owner: u64) -> crate::process::ProcessInstanceId {
        let id = crate::process::ProcessInstanceId(9_001);
        let mut slot = crate::process::TrackProcessSlot::new(id, "lane-count".to_string());
        slot.owner_instance = Some(owner);
        let mut chain = app.state.track_process_chain(0).unwrap();
        chain.slots.push(slot);
        assert!(app.state.set_track_process_chain(0, chain));
        id
    }

    fn owner_of(app: &App, slot: crate::process::ProcessInstanceId) -> Option<u64> {
        app.state
            .track_process_chain(0)
            .unwrap()
            .slots
            .iter()
            .find(|candidate| candidate.instance_id == slot)
            .and_then(|candidate| candidate.owner_instance)
    }

    #[test]
    fn owner_ids_are_reserved_cleared_on_delete_and_restored_on_undo() {
        let mut rig = Rig::new();
        let app = rig.app();
        two_banks(app);
        let _kind = conductor_kind(app);
        // An id a slot still names is never handed out again.
        let stale = app.instances.next_id.max(1);
        owned_slot(app, stale);
        let fresh = create_in(app, None);
        assert_ne!(fresh, stale, "the allocator skips owner ids");
        app.state.set_track_process_chain(0, crate::process::TrackProcessChain::default());
        // Delete clears what the instance owns; undo gives it back.
        let slot = owned_slot(app, fresh);
        app.delete_instance_recorded(fresh).unwrap();
        assert_eq!(owner_of(app, slot), None, "a deleted owner's slot is unowned");
        applied(crate::app::edit::undo(app));
        assert_eq!(owner_of(app, slot), Some(fresh), "undo restores the owner");
        applied(crate::app::edit::redo(app));
        assert_eq!(owner_of(app, slot), None);
    }

    #[test]
    fn a_duplicate_owns_the_copies_of_its_sources_node_processes() {
        let mut rig = Rig::new();
        let app = rig.app();
        two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, None);
        let mut slot = crate::process::TrackProcessSlot::new(crate::process::ProcessInstanceId(77), "lane-count".into());
        slot.owner_instance = Some(id);
        let mut node: crate::graph::ProjectGraphNodeIntrinsicOverride =
            serde_json::from_value(serde_json::json!({"group": "nrn", "instance": 0})).unwrap();
        node.process_chain = Some(crate::process::TrackProcessChain { slots: vec![slot] });
        let graph = crate::graph::ProjectGraphOverrides {
            sequencer_id: id,
            node_intrinsics: vec![node],
            ..Default::default()
        };
        app.state.with_scenes_mut(|scenes| scenes.scenes[0].graph_overrides.push(graph));
        let copy = app.duplicate_instance_recorded(id).unwrap();
        let owners: Vec<Option<u64>> = app.state.with_project_scenes(|scenes| {
            scenes.scenes[0]
                .graph_overrides
                .iter()
                .filter(|graph| graph.sequencer_id == copy)
                .flat_map(|graph| graph.node_intrinsics.iter())
                .filter_map(|node| node.process_chain.as_ref())
                .flat_map(|chain| chain.slots.iter().map(|slot| slot.owner_instance))
                .collect()
        });
        assert_eq!(owners, vec![Some(copy)]);
    }

    #[test]
    fn moving_an_instance_applies_the_default_active_rule_and_a_kit_forgets_it() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let (group_id, _) = app.create_drum_rack_recorded(None).expect("a kit");
        let id = create_in(app, None);
        assert_eq!(parked_scenes(app, id), vec![false, false, true, true]);
        app.move_instance_owner_in_bank_recorded(id, ProjectInstanceOwner::Rack(group_id), None).unwrap();
        assert_eq!(parked_scenes(app, id), vec![false; 4], "a kit's instance follows the kit");
        app.move_instance_owner_in_bank_recorded(id, ProjectInstanceOwner::Project, Some(bank_b)).unwrap();
        assert_eq!(parked_scenes(app, id), vec![true, true, false, false], "given back in bank B");
        applied(crate::app::edit::undo(app));
        assert_eq!(parked_scenes(app, id), vec![false; 4], "one undo step");
    }

    #[test]
    fn a_dissolved_kits_sequencer_plays_where_the_kit_played() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let (group_id, _) = app.create_drum_rack_recorded(None).expect("a kit");
        let id = app
            .create_instance_in_bank_recorded(KIND, ProjectInstanceOwner::Rack(group_id), None, None)
            .unwrap();
        app.set_kit_parked_in_bank_recorded(group_id, true, Some(bank_b)).unwrap();
        app.with_rack_instances_released(&[group_id], "Dissolve", |_| Ok(())).unwrap();
        assert_eq!(app.instances.get(id).unwrap().owner, ProjectInstanceOwner::Project);
        assert_eq!(parked_scenes(app, id), vec![false, false, true, true]);
    }

    #[test]
    fn a_recall_that_changes_nothing_records_nothing_and_undo_restores_overrides() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (bank_a, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let id = create_in(app, Some(bank_a));
        let graph = crate::graph::ProjectGraphOverrides {
            sequencer_id: id,
            max_poly: Some(3),
            ..Default::default()
        };
        app.state.with_scenes_mut(|scenes| scenes.scenes[0].graph_overrides.push(graph.clone()));
        // Bank B takes scene 0's override; undo takes it away again.
        let len = app.history.undo_len();
        assert!(app.activate_instance_in_bank_from_scene_recorded(id, Some(bank_b), 0).unwrap());
        assert_eq!(app.history.undo_len(), len + 1, "one step");
        let in_b = |app: &App| -> Vec<Option<u32>> {
            app.state.with_project_scenes(|s| {
                (2..4)
                    .map(|scene| {
                        s.scenes[scene]
                            .graph_overrides
                            .iter()
                            .find(|graph| graph.sequencer_id == id)
                            .and_then(|graph| graph.max_poly)
                    })
                    .collect()
            })
        };
        assert_eq!(in_b(app), vec![Some(3), Some(3)]);
        // The same recall again changes nothing and records nothing.
        assert!(!app.activate_instance_in_bank_from_scene_recorded(id, Some(bank_b), 0).unwrap());
        assert_eq!(app.history.undo_len(), len + 1);
        applied(crate::app::edit::undo(app));
        assert_eq!(in_b(app), vec![None, None], "undo restores bank B's overrides");
        assert_eq!(parked_scenes(app, id), vec![false, false, true, true]);
    }

    #[test]
    fn a_rack_owned_instance_cannot_be_switched_per_bank() {
        let mut rig = Rig::new();
        let app = rig.app();
        let (_, bank_b) = two_banks(app);
        let _kind = conductor_kind(app);
        let (group_id, _) = app.create_drum_rack_recorded(None).expect("a kit");
        let id = app
            .create_instance_in_bank_recorded(KIND, ProjectInstanceOwner::Rack(group_id), None, Some(bank_b))
            .unwrap();
        assert_eq!(parked_scenes(app, id), vec![false; 4], "a kit's instance follows the kit");
        assert!(app.set_instance_parked_in_bank_recorded(id, true, Some(bank_b)).is_err());
    }
}
