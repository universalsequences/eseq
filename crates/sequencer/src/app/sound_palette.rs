//! Sound palette gestures (takes spec §17.6 / §18.3): Apply / Apply-with-mix
//! / Fork / Rename / Cleanup over the per-track Patch/Mix pools.
//!
//! Every gesture that repoints refs routes through the single S2 seam
//! (`App::after_sound_repoint`) via `commit_sound_relink` — bypassing it
//! silently kills p-locks/key locks and engaged macro overrides. Each
//! gesture is one undo entry (§17.4).

use crate::sequencer::{MixId, PatchId, PatternId, SoundRefs, TakeId, SOUND_COLOR_SET};

use super::sound_binding::BoundSource;
use super::App;

/// The per-track sound color set (§17.11), indexed by
/// `SoundEntityMeta::color`. Same visual family as the p-lock
/// `VARIANT_PALETTE` (the palette mirrors that UI's language), extended to
/// `SOUND_COLOR_SET` entries.
pub const SOUND_PALETTE_RGB: [[f32; 3]; SOUND_COLOR_SET as usize] = [
    [0.270_588_25, 0.784_313_74, 0.862_745_1],
    [0.909_803_9, 0.643_137_3, 0.309_803_93],
    [0.662_745_1, 0.494_117_65, 0.909_803_9],
    [0.435_294_12, 0.807_843_15, 0.541_176_5],
    [0.909_803_9, 0.415_686_28, 0.415_686_28],
    [0.850_980_4, 0.788_235_3, 0.352_941_2],
    [0.396_078_44, 0.560_784_34, 0.921_568_63],
    [0.878_431_4, 0.501_960_8, 0.741_176_5],
];

/// One palette overlay row (§17.6): a Patch-identified entry with its
/// display metadata and the reverse referent index.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    pub patch: PatchId,
    /// The Mix paired with this patch by its first referent — the pair
    /// Apply-with-mix re-links. `None` for a library orphan whose pairing
    /// is unknown (Apply-with-mix is disabled there).
    pub mix: Option<MixId>,
    pub name: String,
    pub color: Option<u8>,
    /// "Scene 1, Pattern 5, Take 2" — the reverse index of the refs, or
    /// "unused" for a library orphan (§17.4 palette-as-library).
    pub referents: String,
    /// The scene-effective sound (§17.6): rendered as the gray base entry.
    pub is_base: bool,
    /// The open target's current patch.
    pub is_current: bool,
}

/// What a palette gesture re-links (§17.6). `Cell` means "the track's
/// effective sound here and now": under an active track-pattern launch that
/// resolves to the OVERRIDE pattern's entity — what you hear — never the
/// hidden scene-cell sound beneath it (§17.3's launch-override deviation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteTarget {
    Take(TakeId),
    Pattern(PatternId),
    Cell,
}

/// A resolved target: the referent lists a masked re-link takes, plus the
/// refs the target holds today (the fork source).
pub(crate) struct ResolvedPaletteTarget {
    pub patterns: Vec<PatternId>,
    pub takes: Vec<TakeId>,
    pub cells: Vec<usize>,
    pub current: SoundRefs,
}

impl App {
    pub(crate) fn resolve_palette_target(
        &self,
        track: usize,
        target: PaletteTarget,
    ) -> Result<ResolvedPaletteTarget, String> {
        match target {
            PaletteTarget::Take(id) => {
                let sound = self
                    .state
                    .with_project_scenes(|scenes| {
                        scenes
                            .take_pools
                            .get(track)
                            .and_then(|takes| takes.get(id))
                            .map(|take| take.sound)
                    })
                    .ok_or_else(|| {
                        format!("Take {} does not exist on track {}", id.0, track + 1)
                    })?;
                Ok(ResolvedPaletteTarget {
                    patterns: Vec::new(),
                    takes: vec![id],
                    cells: Vec::new(),
                    current: sound,
                })
            }
            PaletteTarget::Pattern(id) => {
                let refs = self
                    .state
                    .with_project_scenes(|scenes| {
                        scenes.track_pools.get(track).and_then(|pool| pool.refs(id))
                    })
                    .ok_or_else(|| {
                        format!("Pattern {} does not exist on track {}", id.0, track + 1)
                    })?;
                Ok(ResolvedPaletteTarget {
                    patterns: vec![id],
                    takes: Vec::new(),
                    cells: Vec::new(),
                    current: refs,
                })
            }
            PaletteTarget::Cell => self.state.with_project_scenes(|scenes| {
                // Effective resolution (§17.3 via the launch override):
                // an active track-pattern launch wins, so the gesture hits
                // the entity that is actually sounding.
                if let Some(id) = scenes.effective_pattern_id(track) {
                    let refs = scenes
                        .track_pools
                        .get(track)
                        .and_then(|pool| pool.refs(id))
                        .ok_or_else(|| {
                            format!("The effective pattern on track {} has no sound", track + 1)
                        })?;
                    return Ok(ResolvedPaletteTarget {
                        patterns: vec![id],
                        takes: Vec::new(),
                        cells: Vec::new(),
                        current: refs,
                    });
                }
                // Bare cell (§17.2 "no steps ≠ no sound"): the cell's own
                // refs are the target.
                let scene = scenes.current_scene;
                let refs = scenes
                    .scenes
                    .get(scene)
                    .and_then(|scene| scene.cell_sounds.get(track))
                    .copied()
                    .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
                Ok(ResolvedPaletteTarget {
                    patterns: Vec::new(),
                    takes: Vec::new(),
                    cells: vec![scene],
                    current: refs,
                })
            }),
        }
    }

    /// The palette target the overlay was opened on, defaulting to the
    /// track's bound source (badge-open) when none is stored.
    pub fn palette_target_or_binding(
        &self,
        track: usize,
        target: Option<PaletteTarget>,
    ) -> PaletteTarget {
        target.unwrap_or_else(|| match self.track_sound_binding(track).source {
            Some(BoundSource::Take(id)) => PaletteTarget::Take(id),
            Some(BoundSource::Pattern(id)) => PaletteTarget::Pattern(id),
            None => PaletteTarget::Cell,
        })
    }

    /// **Apply** (§17.6): re-link the target's `patch_ref` to `patch` —
    /// reference semantics, future edits to that patch follow. With
    /// `mix: Some`, the explicit **Apply with mix** variant.
    pub fn palette_apply(
        &mut self,
        track: usize,
        target: PaletteTarget,
        patch: PatchId,
        mix: Option<MixId>,
    ) -> Result<String, String> {
        let resolved = self.resolve_palette_target(track, target)?;
        let label = if mix.is_some() {
            "Apply sound with mix"
        } else {
            "Apply sound"
        };
        self.commit_sound_relink(
            track,
            &resolved.patterns,
            &resolved.takes,
            &resolved.cells,
            Some(patch),
            mix,
            label,
        )
    }

    /// **Fork** (§17.3 "own parameters"): clone the target's entities and
    /// repoint the target at the clones, as one undo entry.
    pub fn palette_fork(&mut self, track: usize, target: PaletteTarget) -> Result<String, String> {
        let resolved = self.resolve_palette_target(track, target)?;
        let before = self.capture_synchronized_scene_structure_state()?;
        let forked = self
            .state
            .fork_track_sound(track, resolved.current)
            .ok_or_else(|| "The target's sound does not resolve".to_string())?;
        let changed = self.state.relink_track_sound_refs_masked(
            track,
            &resolved.patterns,
            &resolved.takes,
            &resolved.cells,
            Some(forked.patch),
            Some(forked.mix),
        )?;
        debug_assert!(changed > 0, "a fresh fork always moves the target");
        let after = self.state.capture_project_scenes();
        crate::app::edit::finish_active_gesture(self);
        let patch = crate::app::history::SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            "Fork sound",
            None,
            crate::app::history::EditPatch::SceneStructure(patch),
            retained_bytes,
        );
        // A fork is a repoint (§17.10): same seam + row invalidation as a
        // re-link.
        self.after_sound_repoint(track);
        self.invalidate_palette_rows(track, &resolved);
        Ok("Fork sound: the target now owns its parameters".to_string())
    }

    /// **Rename** (§17.11, overlay-only): set one entity's display name.
    /// Exactly one of `patch`/`mix`. No repoint — nothing rebinds.
    pub fn palette_rename(
        &mut self,
        track: usize,
        patch: Option<PatchId>,
        mix: Option<MixId>,
        name: &str,
    ) -> Result<String, String> {
        if name.trim().is_empty() {
            return Err("A sound name cannot be empty".to_string());
        }
        let before = self.capture_synchronized_scene_structure_state()?;
        self.state
            .rename_track_sound_entity(track, patch, mix, name)?;
        let after = self.state.capture_project_scenes();
        crate::app::edit::finish_active_gesture(self);
        let patch = crate::app::history::SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            "Rename sound",
            None,
            crate::app::history::EditPatch::SceneStructure(patch),
            retained_bytes,
        );
        Ok(format!("Renamed to {}", name.trim()))
    }

    /// **Clean up unused** (§17.4): the interactive version of the
    /// save-time prune, scoped to one track. Frees colors for reuse
    /// (§17.11). No repoint — only unreferenced entities drop.
    pub fn palette_cleanup_unused(&mut self, track: usize) -> Result<String, String> {
        let before = self.capture_synchronized_scene_structure_state()?;
        let removed = self.state.prune_unreferenced_sounds_for_track(track);
        if removed == 0 {
            return Ok("Nothing to clean up".to_string());
        }
        let after = self.state.capture_project_scenes();
        crate::app::edit::finish_active_gesture(self);
        let patch = crate::app::history::SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            "Clean up unused sounds",
            None,
            crate::app::history::EditPatch::SceneStructure(patch),
            retained_bytes,
        );
        Ok(format!("Cleaned up {removed} unused entit(ies)"))
    }

    /// The palette overlay's rows for `track` (§17.6): every Patch in the
    /// track's pool, ordered by id, with display metadata and the reverse
    /// referent index. `target` marks which entry `is_current`.
    pub fn sound_palette_entries(
        &self,
        track: usize,
        target: PaletteTarget,
    ) -> Vec<PaletteEntry> {
        let current_patch = self
            .resolve_palette_target(track, target)
            .ok()
            .map(|resolved| resolved.current.patch);
        self.state.with_project_scenes(|scenes| {
            let Some(pool) = scenes.track_pools.get(track) else {
                return Vec::new();
            };
            let base_patch = scenes
                .effective_sound_refs(track)
                .map(|refs| refs.patch);
            // Reverse index: patch → referent names, in scene / pattern /
            // take display order; the first referent's mix is the entry's
            // Apply-with-mix pair.
            let mut referents: std::collections::HashMap<PatchId, Vec<String>> =
                std::collections::HashMap::new();
            let mut paired_mix: std::collections::HashMap<PatchId, MixId> =
                std::collections::HashMap::new();
            for scene in &scenes.scenes {
                if let Some(refs) = scene.cell_sounds.get(track) {
                    referents
                        .entry(refs.patch)
                        .or_default()
                        .push(scene.name.clone());
                    paired_mix.entry(refs.patch).or_insert(refs.mix);
                }
            }
            let takes = scenes.take_pools.get(track);
            let mut pattern_ids: Vec<PatternId> = pool
                .patterns
                .keys()
                .copied()
                .filter(|id| !takes.is_some_and(|takes| takes.is_claimed(*id)))
                .collect();
            pattern_ids.sort_by_key(|id| id.0);
            for id in pattern_ids {
                if let Some(refs) = pool.refs(id) {
                    let names = referents.entry(refs.patch).or_default();
                    let label = format!("Pattern {}", id.0);
                    if !names.contains(&label) {
                        names.push(label);
                    }
                    paired_mix.entry(refs.patch).or_insert(refs.mix);
                }
            }
            if let Some(takes) = takes {
                for take in &takes.takes {
                    referents
                        .entry(take.sound.patch)
                        .or_default()
                        .push(take.name.clone());
                    paired_mix.entry(take.sound.patch).or_insert(take.sound.mix);
                }
            }
            let mut patch_ids: Vec<PatchId> = pool.sounds.patches.keys().copied().collect();
            patch_ids.sort();
            patch_ids
                .into_iter()
                .map(|patch| {
                    let meta = pool.sounds.patch_meta.get(&patch);
                    let names = referents.remove(&patch).unwrap_or_default();
                    PaletteEntry {
                        patch,
                        mix: paired_mix.get(&patch).copied(),
                        name: meta
                            .map(|meta| meta.name.clone())
                            .unwrap_or_else(|| format!("Patch {}", patch.0 + 1)),
                        color: meta.and_then(|meta| meta.color),
                        referents: if names.is_empty() {
                            "unused".to_string()
                        } else {
                            names.join(", ")
                        },
                        is_base: Some(patch) == base_patch,
                        is_current: Some(patch) == current_patch,
                    }
                })
                .collect()
        })
    }

    /// The §16.6 badge, S3 form (§17.5): "Patch A — used by Scene 1,
    /// Scene 3, Take 2" for the track's bound sound. Rides the instrument
    /// panel's `inst` map — never a panel-scope `SEQ.*` read (§16 scope
    /// call: that breaks the *fx* buffer's evaluation).
    pub fn sound_binding_badge(&self, track: usize) -> Option<String> {
        let target = self.palette_target_or_binding(track, None);
        let entry = self
            .sound_palette_entries(track, target)
            .into_iter()
            .find(|entry| entry.is_current)?;
        Some(if entry.referents == "unused" {
            format!("{} — unused", entry.name)
        } else {
            format!("{} — used by {}", entry.name, entry.referents)
        })
    }

    /// Clip-dot join (§17.6, amended): per track, each stored clip's dot
    /// visibility and color — `(clip_id, dot, color)`. The dot is **patch
    /// identity**, not divergence from the live effective refs: every clip
    /// with a resolvable sound shows its patch's color (§17.11: a color
    /// identifies exactly one Patch on the track), unconditionally. Two
    /// rejected variants, both of which made the dots appear/disappear for
    /// reasons invisible to the user: comparing against the scene-effective
    /// refs (the baseline follows the playhead under song playback), and
    /// suppressing single-patch lanes (a take splice can collapse a lane to
    /// one patch and the next punch-in re-cross the threshold, toggling
    /// every dot on the lane at once).
    pub fn song_clip_sounds(&self) -> Vec<Vec<(u64, bool, Option<u8>)>> {
        // Two sequential lock scopes, never nested (arrangement and scenes
        // have no established lock order): first the minimal clip tuples,
        // then the sound resolution.
        let lanes: Vec<Vec<(u64, Option<u64>, Option<u64>)>> =
            self.state.with_committed_arrangement(|arrangement| {
                arrangement
                    .map(|arrangement| {
                        arrangement
                            .track_lanes
                            .iter()
                            .map(|clips| {
                                clips
                                    .iter()
                                    .map(|clip| (clip.id.0, clip.take_id, clip.pattern_id))
                                    .collect()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            });
        if lanes.is_empty() {
            return Vec::new();
        }
        self.state.with_project_scenes(|scenes| {
            lanes
                .into_iter()
                .enumerate()
                .map(|(track, clips)| {
                    let pool = scenes.track_pools.get(track);
                    let takes = scenes.take_pools.get(track);
                    let clip_patch = |take_id: Option<u64>, pattern_id: Option<u64>| {
                        if let Some(take_id) = take_id {
                            takes
                                .and_then(|takes| takes.get(crate::sequencer::TakeId(take_id)))
                                .map(|take| take.sound.patch)
                        } else if let Some(pattern_id) = pattern_id {
                            pool.and_then(|pool| pool.refs(PatternId(pattern_id)))
                                .map(|refs| refs.patch)
                        } else {
                            None
                        }
                    };
                    clips
                        .into_iter()
                        .map(|(clip_id, take_id, pattern_id)| {
                            let patch = clip_patch(take_id, pattern_id);
                            let color = patch.and_then(|patch| {
                                pool.and_then(|pool| pool.sounds.patch_meta.get(&patch))
                                    .and_then(|meta| meta.color)
                            });
                            (clip_id, patch.is_some(), color)
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// Song-row re-preflight for every pattern a palette gesture touched
    /// (mirrors `commit_sound_relink`'s tail).
    fn invalidate_palette_rows(&mut self, track: usize, resolved: &ResolvedPaletteTarget) {
        for pattern in &resolved.patterns {
            self.invalidate_song_rows_for_pattern(track, *pattern);
        }
        let take_chunks: Vec<PatternId> = self.state.with_project_scenes(|scenes| {
            scenes
                .take_pools
                .get(track)
                .map(|pool| {
                    resolved
                        .takes
                        .iter()
                        .filter_map(|id| pool.get(*id))
                        .flat_map(|take| take.chunks.iter().copied())
                        .collect()
                })
                .unwrap_or_default()
        });
        for chunk in take_chunks {
            self.invalidate_song_rows_for_pattern(track, chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sound_binding::tests::app_with_take;
    use crate::sequencer::ClipId;

    /// A second pool pattern with a recognizable patch (instrument default +
    /// a key lock) and mix (volume). Returns `(pattern, refs)`.
    fn distinctive_pattern(app: &crate::app::App) -> (PatternId, SoundRefs) {
        app.state.with_scenes_mut(|scenes| {
            let source = scenes.scenes[0].cells[0].expect("scene cell");
            let mut data = scenes.track_pools[0].get(source).expect("cell data");
            data.instrument_slot.defaults[0] = 0.875;
            data.track_params.volume = 0.31;
            // Key locks ride the Patch (§17.12): a per-note lock inside the
            // slot data must travel with an Apply.
            data.instrument_slot
                .key_locks
                .insert(60, vec![Some(0.42)]);
            let id = scenes.track_pools[0].insert(data);
            (id, scenes.track_pools[0].refs(id).expect("fresh refs"))
        })
    }

    /// §18.3 flagship: "grab pattern 3's patch for this take" — steps
    /// untouched, mix untouched, the take sounds like the pattern, and the
    /// key locks came along. One undo entry restores the previous ref.
    #[test]
    fn apply_grabs_a_patch_for_a_take_leaving_steps_and_mix_alone() {
        let (mut app, take, _scene_pattern, chunks) = app_with_take();
        let (_donor, donor_refs) = distinctive_pattern(&app);
        let (before_refs, before_bits, before_volume) =
            app.state.with_project_scenes(|scenes| {
                let take = scenes.take_pools[0].get(take).expect("take");
                let chunk = scenes.track_pools[0].get(chunks[0]).expect("chunk");
                (take.sound, chunk.track_bits, chunk.track_params.volume)
            });
        assert_ne!(before_refs.patch, donor_refs.patch);

        let depth = app.history.undo_len();
        app.palette_apply(0, PaletteTarget::Take(take), donor_refs.patch, None)
            .expect("apply succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");

        app.state.with_project_scenes(|scenes| {
            let take = scenes.take_pools[0].get(take).expect("take");
            assert_eq!(take.sound.patch, donor_refs.patch, "patch re-linked");
            assert_eq!(take.sound.mix, before_refs.mix, "mix untouched");
            for chunk in &chunks {
                assert_eq!(
                    scenes.track_pools[0].refs(*chunk),
                    Some(take.sound),
                    "chunks share the re-linked pair"
                );
            }
            let chunk = scenes.track_pools[0].get(chunks[0]).expect("chunk");
            assert_eq!(chunk.track_bits, before_bits, "steps untouched");
            assert_eq!(
                chunk.track_params.volume.to_bits(),
                before_volume.to_bits(),
                "the take keeps its own fader"
            );
            assert_eq!(
                chunk.instrument_slot.defaults[0],
                0.875,
                "the take now sounds like the donor patch"
            );
            assert_eq!(
                chunk.instrument_slot.key_locks.get(&60),
                Some(&vec![Some(0.42)]),
                "key locks ride the Patch (§17.12)"
            );
        });

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        app.state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.take_pools[0].get(take).expect("take").sound,
                before_refs,
                "undo restores the previous ref in one entry"
            );
        });
    }

    /// §17.6 "new scene, keep sound linked": scene create forks; Apply-with-
    /// mix on the new cell re-links to the old scene's entities, after which
    /// an edit through either scene lands in both.
    #[test]
    fn apply_with_mix_relinks_a_new_scene_to_the_old_sound() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let old_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].refs(scene_pattern))
            .expect("scene refs");
        let new_scene = app.state.with_scenes_mut(|scenes| scenes.new_scene());
        let (new_pattern, forked) = app.state.with_project_scenes(|scenes| {
            let id = scenes.scenes[new_scene].cells[0].expect("forked cell");
            (id, scenes.track_pools[0].refs(id).expect("forked refs"))
        });
        assert_ne!(forked.patch, old_refs.patch, "scene create forked");

        app.palette_apply(
            0,
            PaletteTarget::Cell,
            old_refs.patch,
            Some(old_refs.mix),
        )
        .expect("apply with mix succeeds");
        app.state.with_scenes_mut(|scenes| {
            assert_eq!(
                scenes.track_pools[0].refs(new_pattern),
                Some(old_refs),
                "the new cell's pattern shares the old entities"
            );
            assert_eq!(
                scenes.scenes[new_scene].cell_sounds[0], old_refs,
                "the cell followed its pattern"
            );
            // An edit through the NEW scene's pattern is heard by the old.
            assert!(scenes.track_pools[0].edit(new_pattern, |data| {
                data.instrument_slot.defaults[0] = 0.66;
            }));
            assert_eq!(
                scenes.track_pools[0]
                    .get(scene_pattern)
                    .expect("old pattern")
                    .instrument_slot
                    .defaults[0],
                0.66,
                "edits land in both scenes (§17.3 sharing)"
            );
        });
    }

    /// A track-pattern launch is an override, not a cell assignment: a Cell
    /// target must resolve to the OVERRIDE pattern's entity — what you hear
    /// — not the hidden cell sound beneath it.
    #[test]
    fn cell_target_resolves_the_override_pattern_under_a_launch() {
        let (app, _take, scene_pattern, _chunks) = app_with_take();
        let (other, other_refs) = distinctive_pattern(&app);
        assert!(app.state.launch_track_pattern(
            0,
            other,
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[crate::sequencer::InstrumentType::Sampler],
        ));
        let resolved = app
            .resolve_palette_target(0, PaletteTarget::Cell)
            .expect("cell target resolves");
        assert_eq!(
            resolved.patterns,
            vec![other],
            "the override pattern is the deliberate target"
        );
        assert_eq!(resolved.current, other_refs);
        let scene_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].refs(scene_pattern));
        assert_ne!(Some(resolved.current), scene_refs);
    }

    /// Fork while a take is bound (audible-class): one undo entry, and undo
    /// restores the shared ref.
    #[test]
    fn fork_owns_parameters_and_undoes_in_one_entry() {
        let (mut app, take, _scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        let before = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);
        let depth = app.history.undo_len();
        app.palette_fork(0, PaletteTarget::Take(take))
            .expect("fork succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");
        let after = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);
        assert_ne!(after, before, "the take owns fresh entities");
        app.state.with_project_scenes(|scenes| {
            for chunk in &chunks {
                assert_eq!(scenes.track_pools[0].refs(*chunk), Some(after));
            }
            // The fork is a value copy of the source.
            let forked = scenes.track_pools[0].get(chunks[0]).expect("chunk");
            let source = scenes.track_pools[0]
                .compose_bare_sound(before)
                .expect("source resolves");
            assert_eq!(
                forked.instrument_slot.defaults[0],
                source.instrument_slot.defaults[0]
            );
        });
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let restored = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);
        assert_eq!(restored, before, "undo restores the shared ref");
    }

    /// Rename + cleanup: rename is overlay-metadata only; cleanup drops
    /// exactly the unreferenced entities and frees their colors (§17.11).
    #[test]
    fn rename_and_cleanup_manage_display_metadata() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        let refs = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);
        app.palette_rename(0, Some(refs.patch), None, "Warm Keys")
            .expect("rename succeeds");
        let entries = app.sound_palette_entries(0, PaletteTarget::Take(take));
        let entry = entries
            .iter()
            .find(|entry| entry.patch == refs.patch)
            .expect("entry exists");
        assert_eq!(entry.name, "Warm Keys");
        assert!(entry.is_current);
        assert!(
            entry.referents.contains("Take"),
            "referent index names the take: {}",
            entry.referents
        );

        // Orphan an entity, then clean up: only the orphan drops.
        let (donor, donor_refs) = distinctive_pattern(&app);
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.delete_track_pattern(0, donor));
        });
        let before = app.sound_palette_entries(0, PaletteTarget::Take(take)).len();
        app.palette_cleanup_unused(0).expect("cleanup succeeds");
        let after = app.sound_palette_entries(0, PaletteTarget::Take(take));
        assert_eq!(after.len(), before - 1, "exactly the orphan dropped");
        assert!(after.iter().all(|entry| entry.patch != donor_refs.patch));
    }

    /// Clip dots are patch IDENTITY, not divergence from the (playhead-
    /// following) effective refs: EVERY clip with a resolvable sound is
    /// dotted with its own patch color, unconditionally — nothing here
    /// reads the current scene or thresholds on the lane's patch count, so
    /// neither playback row transitions nor take splices can toggle dots.
    #[test]
    fn clip_dots_mark_patch_identity_and_ignore_the_effective_refs() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        let lanes = app.song_clip_sounds();
        assert!(
            lanes[0].iter().all(|(_, dot, _)| *dot),
            "a single-patch lane still dots its clips: {lanes:?}"
        );

        // A second clip with a different patch: both clips show dots.
        let (donor, donor_refs) = distinctive_pattern(&app);
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 32.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new_take(
            ClipId(0),
            0.0,
            16.0,
            take.0,
            0.0,
        ));
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new(
            ClipId(1),
            16.0,
            32.0,
            Some(donor.0),
        ));
        arrangement.next_clip_id = 2;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");

        let lanes = app.song_clip_sounds();
        assert_eq!(lanes[0].len(), 2);
        assert!(
            lanes[0].iter().all(|(_, dot, _)| *dot),
            "every clip on a multi-patch lane is dotted: {lanes:?}"
        );
        let donor_color = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .sounds
                .patch_meta
                .get(&donor_refs.patch)
                .and_then(|meta| meta.color)
        });
        assert_eq!(
            lanes[0][1].2, donor_color,
            "the dot carries the clip's own patch color"
        );
        assert_ne!(lanes[0][0].2, lanes[0][1].2, "colors differ per patch");
    }

    /// The badge (§17.5): "Patch A — used by …" for the bound sound.
    #[test]
    fn badge_names_the_bound_patch_and_its_referents() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        let refs = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);
        app.palette_rename(0, Some(refs.patch), None, "Warm Keys")
            .expect("rename succeeds");
        let badge = app.sound_binding_badge(0).expect("badge exists");
        assert!(
            badge.starts_with("Warm Keys — used by "),
            "unexpected badge: {badge}"
        );
        assert!(badge.contains("Take"), "unexpected badge: {badge}");
    }
}
