//! Sound palette gestures (takes spec §17.6 / §18.3): Apply / Apply-with-mix
//! / Fork / Rename / Cleanup over the per-track Patch/Mix pools.
//!
//! Every gesture that repoints refs routes through the single S2 seam
//! (`App::after_sound_repoint`) via `commit_sound_relink` — bypassing it
//! silently kills p-locks/key locks and engaged macro overrides. Each
//! gesture is one undo entry (§17.4).

use crate::sequencer::{
    InstrumentType, MixId, PatchId, PatternId, SoundRefs, TakeId, SOUND_COLOR_SET,
};

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
    /// The same index abbreviated for the compact palette card:
    /// "S1 P5 T2".
    pub referents_short: String,
    /// The scene-effective sound (§17.6): rendered as the gray base entry.
    pub is_base: bool,
    /// This Patch IS the track's own sound (track-sound spec §2.1): its refs
    /// match `track_sound_refs(track)`. The carrier PATTERN is hidden from
    /// pattern listings, but its Patch/Mix entities are ordinary pool
    /// entities that render as palette cards — this flag is what identifies
    /// them (rendered as the card's "TRK" chip). Takes that share the track
    /// sound (§2.4.1) reference the SAME pair, so a card can carry both take
    /// referents and this flag at once.
    pub is_track_sound: bool,
    /// The open target's current patch.
    pub is_current: bool,
    /// The instrument preset the patch was loaded from (a `*` suffix marks
    /// it edited since), when one is known.
    pub preset: Option<String>,
    /// The loaded sample's name for a sampler patch — the sampler
    /// equivalent of a preset name.
    pub sample: Option<String>,
    /// Git-diff-style summary vs the open target's current patch: how many
    /// visible instrument params sit higher / lower than the current sound.
    /// Both zero for the current entry itself and for incompatible patches.
    pub params_up: usize,
    pub params_down: usize,
}

/// "Scene 12" → "S12", "Pattern 5" → "P5", "Take 2" → "T2"; a custom name
/// keeps its first 6 chars.
fn short_referent(name: &str) -> String {
    for (prefix, letter) in [("Scene ", "S"), ("Pattern ", "P"), ("Take ", "T")] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return format!("{letter}{rest}");
        }
    }
    name.chars().take(6).collect()
}

/// Mirror of the glyph pipeline's hidden-param filter: modulation plumbing
/// and `hidden`/`ui`/`non-audio`-tagged params don't count toward the diff.
fn diffable_param(param: &crate::effects::ParamDescriptor) -> bool {
    let plumbing = param.name.starts_with("__dgen_mod_active__")
        || (param.name.starts_with("mod ")
            && param.name.contains(" slot ")
            && param.name.ends_with(" amt"));
    !plumbing
        && !param.ui_metadata.as_ref().is_some_and(|metadata| {
            metadata
                .tags
                .iter()
                .any(|tag| matches!(tag.as_str(), "hidden" | "ui" | "non-audio"))
        })
}

/// Count how many visible params sit higher / lower in `candidate` than in
/// `current`. The caller guarantees the two default vectors describe the
/// same instrument (equal lengths).
fn param_diff_counts(
    descriptor: Option<&crate::effects::EffectDescriptor>,
    current: &[f32],
    candidate: &[f32],
) -> (usize, usize) {
    let mut up = 0;
    let mut down = 0;
    for (index, (current, candidate)) in current.iter().zip(candidate).enumerate() {
        if descriptor
            .and_then(|descriptor| descriptor.params.get(index))
            .is_some_and(|param| !diffable_param(param))
        {
            continue;
        }
        if candidate > current {
            up += 1;
        } else if candidate < current {
            down += 1;
        }
    }
    (up, down)
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
                // View-keyed ownership (track-sound spec §2.2.2): on a
                // track-owned lane — arrangement context, rules 1/2
                // unclaimed — the TRACK SOUND is the effective sound. The
                // current scene's cell is inert-but-visible there, so the
                // gesture must never repoint it: the user hears (and edits)
                // the track sound, and a fork/apply that silently moved the
                // cell instead would diverge the palette from the monitor.
                // An override pin is exempt: a track-pattern launch is a
                // rule-2 claim and its pattern is what is actually sounding
                // (§17.3's launch-override deviation below).
                let pinned = scenes
                    .track_overrides
                    .get(track)
                    .copied()
                    .flatten()
                    .is_some();
                let track_owned = track < 64
                    && self.state.track_owned_lane_mask() >> track & 1 == 1
                    && !pinned;
                if !track_owned {
                    // Effective resolution (§17.3 via the launch override):
                    // an active track-pattern launch wins, so the gesture
                    // hits the entity that is actually sounding.
                    if let Some(id) = scenes.effective_pattern_id(track) {
                        let refs = scenes
                            .track_pools
                            .get(track)
                            .and_then(|pool| pool.refs(id))
                            .ok_or_else(|| {
                                format!(
                                    "The effective pattern on track {} has no sound",
                                    track + 1
                                )
                            })?;
                        return Ok(ResolvedPaletteTarget {
                            patterns: vec![id],
                            takes: Vec::new(),
                            cells: Vec::new(),
                            current: refs,
                        });
                    }
                }
                // Bare or track-owned lane (track-sound spec §2.2 rule 3b):
                // the TRACK SOUND is the target — it is what the lane monitors,
                // edits, and records with, and unlike the cell's refs it
                // does not flap when arrangement playback moves
                // `current_scene`. The carrier pattern rides the ordinary
                // pattern re-link path.
                if let (Some(id), Some(refs)) = (
                    scenes.track_sound_pattern(track),
                    scenes.track_sound_refs(track),
                ) {
                    return Ok(ResolvedPaletteTarget {
                        patterns: vec![id],
                        takes: Vec::new(),
                        cells: Vec::new(),
                        current: refs,
                    });
                }
                // Fallback for out-of-invariant states: the cell's own refs.
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
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("A sound name cannot be empty".to_string());
        }
        // A same-name commit would burn an undo entry on a no-op.
        let unchanged = self.state.with_project_scenes(|scenes| {
            scenes
                .track_pools
                .get(track)
                .is_some_and(|pool| match (patch, mix) {
                    (Some(id), None) => pool
                        .sounds
                        .patch_meta
                        .get(&id)
                        .is_some_and(|meta| meta.name == trimmed),
                    (None, Some(id)) => pool
                        .sounds
                        .mix_meta
                        .get(&id)
                        .is_some_and(|meta| meta.name == trimmed),
                    _ => false,
                })
        });
        if unchanged {
            return Ok(format!("Already named {trimmed}"));
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
        Ok(format!("Renamed to {trimmed}"))
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
        let descriptor = self.graph.instrument_descriptors.get(track);
        self.state.with_project_scenes(|scenes| {
            let Some(pool) = scenes.track_pools.get(track) else {
                return Vec::new();
            };
            let base_patch = scenes
                .effective_sound_refs(track)
                .map(|refs| refs.patch);
            // The track's own sound (track-sound spec §2.1): the pair the
            // carrier pattern references. Marks its Patch's card ("TRK").
            let carrier = scenes.track_sound_pattern(track);
            let track_sound_patch = scenes.track_sound_refs(track).map(|refs| refs.patch);
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
                    // The hidden carrier pattern (§2.1) is not a
                    // user-visible pattern: it must not mint a "Pattern N"
                    // referent chip — the entry's `is_track_sound` flag
                    // marks its entities instead. Its pairing still anchors
                    // Apply-with-mix.
                    if Some(id) != carrier {
                        let names = referents.entry(refs.patch).or_default();
                        let label = format!("Pattern {}", id.0);
                        if !names.contains(&label) {
                            names.push(label);
                        }
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
            let current_defaults = current_patch
                .and_then(|patch| pool.sounds.patches.get(&patch))
                .map(|patch| patch.instrument_slot.defaults.clone());
            patch_ids
                .into_iter()
                .map(|patch| {
                    let meta = pool.sounds.patch_meta.get(&patch);
                    let names = referents.remove(&patch).unwrap_or_default();
                    let is_track_sound = Some(patch) == track_sound_patch;
                    let data = pool.sounds.patches.get(&patch);
                    let (params_up, params_down) = match (&current_defaults, data) {
                        (Some(current), Some(data))
                            if Some(patch) != current_patch
                                && data.instrument_slot.defaults.len() == current.len() =>
                        {
                            param_diff_counts(descriptor, current, &data.instrument_slot.defaults)
                        }
                        _ => (0, 0),
                    };
                    PaletteEntry {
                        patch,
                        mix: paired_mix.get(&patch).copied(),
                        name: meta
                            .map(|meta| meta.name.clone())
                            .unwrap_or_else(|| format!("Patch {}", patch.0 + 1)),
                        color: meta.and_then(|meta| meta.color),
                        referents: if names.is_empty() {
                            // The track sound is never "unused": the carrier
                            // references it even when nothing else does.
                            if is_track_sound {
                                "track sound".to_string()
                            } else {
                                "unused".to_string()
                            }
                        } else {
                            names.join(", ")
                        },
                        referents_short: if names.is_empty() {
                            // The card's TRK chip already says it; an empty
                            // referent line avoids "TRK track" duplication.
                            if is_track_sound {
                                String::new()
                            } else {
                                "unused".to_string()
                            }
                        } else {
                            names
                                .iter()
                                .map(|name| short_referent(name))
                                .collect::<Vec<_>>()
                                .join(" ")
                        },
                        is_base: Some(patch) == base_patch,
                        is_track_sound,
                        is_current: Some(patch) == current_patch,
                        preset: data
                            .and_then(|data| data.track_sound_state.loaded_preset.as_deref())
                            .filter(|name| !name.is_empty())
                            .map(|name| {
                                let dirty =
                                    data.is_some_and(|data| data.track_sound_state.dirty);
                                if dirty {
                                    format!("{name}*")
                                } else {
                                    name.to_string()
                                }
                            }),
                        sample: data
                            .filter(|data| data.instrument_type == InstrumentType::Sampler)
                            .map(|data| data.sample_id.1.clone())
                            .filter(|name| !name.is_empty()),
                        params_up,
                        params_down,
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
        // A scene workflow: in Seq view the Cell target is the scene's cell
        // (§2.2.2 — in arrangement view it would be the track sound).
        app.arrangement_view_visible = false;
        app.state.set_arrangement_context(false);
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
        // A same-name rename (whitespace included) is a no-op: no undo entry.
        let depth = app.history.undo_len();
        app.palette_rename(0, Some(refs.patch), None, "  Warm Keys ")
            .expect("no-op rename succeeds");
        assert_eq!(
            app.history.undo_len(),
            depth,
            "a same-name rename commits no undo entry"
        );
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
        let (app, take, _scene_pattern, _chunks) = app_with_take();
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

    /// Track-sound spec §2.2.2 ("the clone poisons the sequence",
    /// 2026-08-06): in ARRANGEMENT view with nothing selected and nothing
    /// latched, the lane is track-owned and the palette's Cell target IS the
    /// track sound — the pair the user hears and edits. Pre-fix the target
    /// resolved the current scene's inert-but-visible cell, so "+" silently
    /// repointed the SESSION cell at a frozen clone while the audible track
    /// sound stayed shared: the palette claimed divergence that never
    /// happened, and the scene came back re-sounded.
    #[test]
    fn fork_on_a_track_owned_lane_forks_the_track_sound_not_the_inert_cell() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        assert_eq!(
            app.state.track_owned_lane_mask() & 1,
            1,
            "rules 1/2 unclaimed in arrangement view: the track owns the lane"
        );
        let (carrier_before, cell_refs_before, cell_bits_before, take_sound_before) =
            app.state.with_project_scenes(|scenes| {
                (
                    scenes.track_sound_refs(0).expect("track sound resolves"),
                    scenes.track_pools[0]
                        .refs(scene_pattern)
                        .expect("cell refs"),
                    scenes.track_pools[0]
                        .get(scene_pattern)
                        .expect("cell data")
                        .track_bits,
                    scenes.take_pools[0].get(take).expect("take").sound,
                )
            });

        let target = app.palette_target_or_binding(0, None);
        assert_eq!(target, PaletteTarget::Cell, "badge-open target is rule 3");
        app.palette_fork(0, target).expect("fork succeeds");

        app.state.with_project_scenes(|scenes| {
            let carrier_after = scenes.track_sound_refs(0).expect("track sound resolves");
            assert_ne!(
                carrier_after, carrier_before,
                "the TRACK SOUND forked — the target the user hears"
            );
            assert_eq!(
                scenes.track_pools[0].refs(scene_pattern),
                Some(cell_refs_before),
                "the inert session cell keeps its own sound"
            );
            assert_eq!(
                scenes.track_pools[0]
                    .get(scene_pattern)
                    .expect("cell data")
                    .track_bits,
                cell_bits_before,
                "the cell's note content is untouched"
            );
            assert_eq!(
                scenes.take_pools[0].get(take).expect("take").sound,
                take_sound_before,
                "un-cloned referents keep their refs"
            );
            // The fork is a value copy of what was sounding.
            let forked = scenes.track_pools[0]
                .compose_bare_sound(carrier_after)
                .expect("fork resolves");
            let source = scenes.track_pools[0]
                .compose_bare_sound(carrier_before)
                .expect("source resolves");
            assert_eq!(
                forked.instrument_slot.defaults,
                source.instrument_slot.defaults,
                "the clone sounds like what the user heard"
            );
        });
    }

    /// The Seq-view half of §2.2.2: the classic scene+pattern world is
    /// untouched — a Cell fork targets the scene's effective pattern, the
    /// track sound stays dormant, and the pattern's note content survives.
    #[test]
    fn fork_in_seq_view_targets_the_scene_cell_and_spares_the_track_sound() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        app.arrangement_view_visible = false;
        app.state.set_arrangement_context(false);
        let (carrier_before, cell_refs_before, cell_bits_before) =
            app.state.with_project_scenes(|scenes| {
                (
                    scenes.track_sound_refs(0).expect("track sound resolves"),
                    scenes.track_pools[0]
                        .refs(scene_pattern)
                        .expect("cell refs"),
                    scenes.track_pools[0]
                        .get(scene_pattern)
                        .expect("cell data")
                        .track_bits,
                )
            });

        app.palette_fork(0, PaletteTarget::Cell).expect("fork succeeds");

        app.state.with_project_scenes(|scenes| {
            let cell_refs_after = scenes.track_pools[0]
                .refs(scene_pattern)
                .expect("cell refs");
            assert_ne!(cell_refs_after, cell_refs_before, "the cell forked");
            assert_eq!(
                scenes.scenes[scenes.current_scene].cell_sounds[0], cell_refs_after,
                "the cell followed its pattern"
            );
            assert_eq!(
                scenes.track_sound_refs(0),
                Some(carrier_before),
                "the track sound is dormant in Seq view"
            );
            assert_eq!(
                scenes.track_pools[0]
                    .get(scene_pattern)
                    .expect("cell data")
                    .track_bits,
                cell_bits_before,
                "the pattern's note content is untouched"
            );
        });
    }

    /// §2.2 rule 3b on a genuinely bare lane in arrangement view: "+" forks
    /// the track sound, the carrier repoints at the clone, and the mirror
    /// keeps sounding exactly what the user heard.
    #[test]
    fn fork_on_a_bare_lane_repoints_the_carrier_and_keeps_the_mirror() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        // Bare lane: drop the one scene cell (the take keeps its chunks).
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.clear_cell(0, 0).is_some());
            assert!(scenes.delete_track_pattern(0, scene_pattern));
        });
        assert!(app.state.effective_track_pattern_id(0).is_none());
        // "The preset": carrier entity and live mirror agree on 0.44.
        let carrier_before = app.state.with_scenes_mut(|scenes| {
            let refs = scenes.track_sound_refs(0).expect("track sound resolves");
            let mut mix = (*scenes.track_pools[0].sounds.mixes[&refs.mix]).clone();
            mix.volume = 0.44;
            scenes.track_pools[0]
                .sounds
                .mixes
                .insert(refs.mix, std::sync::Arc::new(mix));
            refs
        });
        app.state.pattern.track_params[0].set_volume(0.44);
        let take_sound_before = app
            .state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).expect("take").sound);

        app.palette_fork(0, PaletteTarget::Cell).expect("fork succeeds");

        let carrier_after = app
            .state
            .with_project_scenes(|scenes| scenes.track_sound_refs(0))
            .expect("track sound resolves");
        assert_ne!(carrier_after, carrier_before, "the carrier repointed");
        assert_eq!(
            app.state.pattern.track_params[0].get_volume().to_bits(),
            0.44f32.to_bits(),
            "the mirror stays what the user heard"
        );
        app.state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[0].sounds.mixes[&carrier_after.mix]
                    .volume
                    .to_bits(),
                0.44f32.to_bits(),
                "the clone carries the audible device state"
            );
            assert_eq!(
                scenes.track_pools[0].sounds.mixes[&carrier_before.mix]
                    .volume
                    .to_bits(),
                0.44f32.to_bits(),
                "the source entities are unchanged"
            );
            assert_eq!(
                scenes.take_pools[0].get(take).expect("take").sound,
                take_sound_before,
                "the take's binding is untouched by the track-sound fork"
            );
        });
    }

    /// The palette marks which card IS the track's own sound (task: "which
    /// pool sound is the track sound?"): the entry whose refs match
    /// `track_sound_refs` carries `is_track_sound`, the hidden carrier never
    /// mints a bogus "Pattern N" referent chip, and a take that SHARES the
    /// pair (§2.4.1) keeps the flag on the same card.
    #[test]
    fn palette_marks_the_track_sound_entry() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        let carrier_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_sound_refs(0))
            .expect("track sound resolves");
        let entries = app.sound_palette_entries(0, PaletteTarget::Take(take));
        let entry = entries
            .iter()
            .find(|entry| entry.patch == carrier_refs.patch)
            .expect("the track sound's Patch renders as a card");
        assert!(entry.is_track_sound, "the flag marks the pair");
        assert!(
            !entry.referents.contains("Pattern"),
            "the hidden carrier is not a pattern referent: {}",
            entry.referents
        );
        assert_eq!(
            entry.referents, "track sound",
            "a carrier-only pair is never 'unused'"
        );
        assert!(
            entries
                .iter()
                .filter(|entry| entry.is_track_sound)
                .count()
                == 1,
            "exactly one card is the track sound"
        );

        // Take-share nuance (§2.4.1): re-link the take to the track sound's
        // pair — the SAME card now lists the take and keeps the flag.
        app.palette_apply(
            0,
            PaletteTarget::Take(take),
            carrier_refs.patch,
            Some(carrier_refs.mix),
        )
        .expect("apply succeeds");
        let entries = app.sound_palette_entries(0, PaletteTarget::Take(take));
        let entry = entries
            .iter()
            .find(|entry| entry.patch == carrier_refs.patch)
            .expect("the shared card renders");
        assert!(entry.is_track_sound, "sharing does not clear the flag");
        assert!(
            entry.referents.contains("Take"),
            "the take referent joined the card: {}",
            entry.referents
        );
        assert!(entry.is_current, "the take target's current card is the pair");
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
