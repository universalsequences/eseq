//! Scenes and banks, and the transport's scene fields.

use super::*;

impl HostKinds {
    /// Scenes, banks, their fields, and the transport's scene fields.
    /// Returns false when the scene or bank ids were not distinct this frame
    /// (identity never falls back to position; nothing changed then).
    pub(super) fn sync_scene_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let (scene_rows, bank_rows, current) = app.state.with_project_scenes(|project| {
            let scenes: Vec<(u64, String)> = project
                .scenes
                .iter()
                .map(|scene| (scene.id.0, scene.name.clone()))
                .collect();
            let banks: Vec<(u64, Option<String>, usize)> = project
                .scene_banks()
                .iter()
                .map(|bank| (bank.id.0, bank.name.clone(), bank.len))
                .collect();
            (scenes, banks, project.current_scene)
        });
        let scene_model: Vec<u64> = scene_rows.iter().map(|(id, _)| *id).collect();
        let bank_model: Vec<u64> = bank_rows.iter().map(|(id, _, _)| *id).collect();
        if !distinct(&scene_model) || !distinct(&bank_model) {
            return false;
        }
        let queued = queued_transport_scene(&app.state);
        let scenes = reconcile(pusher, SCENE, &mut self.scenes, &scene_model);
        let banks = reconcile(pusher, BANK, &mut self.banks, &bank_model);
        // Each scene's bank and position in it (banks are consecutive spans).
        let mut holder: Vec<Option<(usize, usize)>> = vec![None; scenes.len()];
        let mut offset = 0;
        for (bank, (bid, name, len)) in bank_rows.iter().enumerate() {
            let span = offset..(offset + len).min(scenes.len());
            offset += len;
            for scene in span.clone() {
                holder[scene] = Some((bank, scene - span.start));
            }
            let Some(&Some(id)) = banks.get(bank) else {
                continue;
            };
            pusher.push(id, f::BANK_INDEX, number(bank as f64));
            pusher.push(id, f::BANK_BID, number(*bid as f64));
            pusher.push(
                id,
                f::BANK_NAME,
                Value::String(name.clone().unwrap_or_default()),
            );
            pusher.push(
                id,
                f::BANK_LABEL,
                Value::String(scene_bank_label(bank, name.as_deref())),
            );
            let members = span.clone().filter_map(|scene| scenes[scene]);
            pusher.push(id, f::BANK_SCENES, instance_list(members));
            pusher.push(id, f::BANK_PLAYING, Value::Bool(span.contains(&current)));
        }
        for (scene, id) in scenes.iter().enumerate() {
            let Some(id) = *id else { continue };
            let (bank, number_in_bank) = match holder[scene] {
                Some((bank, position)) => (banks.get(bank).copied().flatten(), position),
                None => (None, scene),
            };
            pusher.push(id, f::SCENE_INDEX, number(scene as f64));
            pusher.push(id, f::SCENE_NUMBER, number((number_in_bank + 1) as f64));
            pusher.push(
                id,
                f::SCENE_NAME,
                Value::String(scene_rows[scene].1.clone()),
            );
            pusher.push(id, f::SCENE_ACTIVE, Value::Bool(scene == current));
            pusher.push(id, f::SCENE_QUEUED, Value::Bool(queued == Some(scene)));
            pusher.push(id, f::SCENE_BANK, instance_or_nil(bank));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(project, f::PROJECT_SCENES, listed_instances(&scenes));
            pusher.push(project, f::PROJECT_BANKS, listed_instances(&banks));
        }
        if let Some(transport) = pusher.singleton(TRANSPORT) {
            let scene = scenes.get(current).copied().flatten();
            pusher.push(transport, f::TRANSPORT_SCENE, instance_or_nil(scene));
            let queued_id = queued.and_then(|scene| scenes.get(scene).copied().flatten());
            pusher.push(transport, f::TRANSPORT_QUEUED, instance_or_nil(queued_id));
        }
        self.scene_ids = scenes;
        self.bank_ids = banks;
        self.queued = Some(queued);
        true
    }

    /// The queued scene and launch quantization, compared every tick (a
    /// quantized launch arms and fires without moving any model counter).
    pub(super) fn sync_transport_queue(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let queued = queued_transport_scene(&app.state);
        if self.queued != Some(queued) {
            let previous = self.queued.flatten();
            for scene in [previous, queued].into_iter().flatten() {
                if let Some(Some(id)) = self.scene_ids.get(scene) {
                    pusher.push(*id, f::SCENE_QUEUED, Value::Bool(queued == Some(scene)));
                }
            }
            if let Some(transport) = pusher.singleton(TRANSPORT) {
                let queued_id =
                    queued.and_then(|scene| self.scene_ids.get(scene).copied().flatten());
                pusher.push(transport, f::TRANSPORT_QUEUED, instance_or_nil(queued_id));
            }
            self.queued = Some(queued);
        }
        let Some(transport) = pusher.singleton(TRANSPORT) else {
            return;
        };
        let quantize = self.scene_launch_quantize;
        if self.launch_quantize != Some(quantize) {
            self.launch_quantize = Some(quantize);
            pusher.push(
                transport,
                f::TRANSPORT_LAUNCH_QUANTIZE,
                Value::String(quantize.transport_label().to_string()),
            );
        }
    }

    /// The scene launch quantization (`transport.launch-quantize`), pushed
    /// at the next sync.
    pub(crate) fn scene_launch_quantize(&self) -> LaunchQuantize {
        self.scene_launch_quantize
    }

    /// Set the scene launch quantization (`set-scene-launch-quantize`).
    pub(crate) fn set_scene_launch_quantize(&mut self, quantize: LaunchQuantize) {
        self.scene_launch_quantize = quantize;
    }
}
