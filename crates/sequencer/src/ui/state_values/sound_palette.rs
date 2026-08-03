//! Sound palette read surfaces (takes spec §17.6 / §18.3):
//! `SEQ.sound-palette` (the open overlay's entries) and
//! `SEQ.song-clip-sounds` (the timeline clip-dot identity join). Both diff by value
//! before publishing, like `scene-names` — the underlying scenes have no
//! revision counter and palette gestures can move refs without touching the
//! committed-song revision.

use super::*;
use crate::app::sound_palette::{PaletteEntry, PaletteTarget, SOUND_PALETTE_RGB};
use eseqlisp::sound_glyph_data::{
    publish_sound_glyph_frames, retain_sound_glyph_frames, SoundGlyphFrame, SoundGlyphPiece,
};
use sequencer::delta_glyph::{
    DeltaGlyphCohort, IdentityBranch, ParamGroup, ParamKind as GlyphParamKind, ParamSchema,
    ParamTaper,
};
use sequencer::effects::{ParamKind, ParamScaling};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct SoundPaletteFrameState {
    /// `(track, target, instrument name, entries)` of the last published
    /// overlay, `None` when the last publish was Nil (closed).
    cached: Option<(usize, PaletteTarget, String, Vec<PaletteEntry>)>,
    /// Whether anything was ever published (so the first closed frame does
    /// not publish Nil over the registered default).
    published_open: bool,
    cached_clip_sounds: Option<Vec<Vec<(u64, bool, Option<u8>)>>>,
    glyphs: GlyphFrames,
}

/// Per-frame cache for cohort-relative glyph frames. The fingerprint includes
/// the whole ordered cohort and reference patch, because either can change
/// every tile's normalized deviation vector.
#[derive(Default)]
struct GlyphFrames {
    published: HashMap<String, u64>,
    revision: u64,
    /// AST identity branches for the delta glyph's identity tier (spec §5.1a),
    /// cached because resolving them reads and parses the instrument's dsp.lisp.
    identity: Option<(String, Vec<IdentityBranch>)>,
}

/// The identity tier's input: branch clusters of the instrument's authored AST,
/// via `extract_skeleton` for custom instruments and the stock skeleton for
/// builtins/samplers. Cached on the instrument name + descriptor param count so
/// an instrument reload with a changed surface refreshes it.
fn identity_branches<'a>(
    app: &app::App,
    track: usize,
    descriptor: &sequencer::effects::EffectDescriptor,
    glyphs: &'a mut GlyphFrames,
) -> &'a [IdentityBranch] {
    let custom = current_custom_instrument_name(app, track);
    let cache_key = format!(
        "{}:{}",
        custom.as_deref().unwrap_or("stock"),
        descriptor.params.len()
    );
    if glyphs.identity.as_ref().map(|(key, _)| key.as_str()) != Some(cache_key.as_str()) {
        let skeleton = custom
            .and_then(|name| sequencer::lisp_host::load_instrument_source(&name).ok())
            .map(|source| sequencer::sound_glyph::extract_skeleton(&source).skeleton)
            .unwrap_or_else(|| sequencer::sound_glyph::stock_skeleton(descriptor).skeleton);
        glyphs.identity = Some((cache_key, sequencer::sound_glyph::identity_branches(&skeleton)));
    }
    glyphs.identity.as_ref().map(|(_, branches)| branches.as_slice()).unwrap_or(&[])
}

fn glyph_key(track: usize, patch: u64) -> String {
    format!("sound-glyph:track:{track}:patch:{patch}")
}

/// Group a parameter for the delta glyph's localization channel.
///
/// Authored `ui_metadata.group` wins, but no instrument in the repo sets it yet, so
/// the fallback matters more than the primary path. It keys off the param name's
/// **leading snake_case token** rather than a substring search over the whole name:
/// substring matching classifies `opa_level_db` as Mix (it contains "level") when it
/// is plainly operator A, and one misfiled parameter miscolors a whole lobe.
fn glyph_group(raw: Option<&str>, param_name: &str) -> ParamGroup {
    if let Some(group) = raw {
        return named_group(&group.trim_start_matches(':').to_ascii_lowercase());
    }
    let name = param_name.to_ascii_lowercase();
    let token = name.split(['_', '.', ' ']).next().unwrap_or(&name);
    named_group(token)
}

fn named_group(token: &str) -> ParamGroup {
    // `opa`/`op1`-style operator prefixes are oscillators, not "other".
    let operator = token.starts_with("op") && token.len() <= 4;
    if operator || matches!(token, "osc" | "voice" | "source" | "carrier" | "tone" | "wave") {
        ParamGroup::Osc
    } else if matches!(token, "filter" | "filt" | "vcf" | "lpf" | "hpf" | "svf" | "cutoff") {
        ParamGroup::Filter
    } else if token.ends_with("env") || matches!(token, "env" | "eg" | "adsr" | "amp" | "attack" | "decay") {
        ParamGroup::Env
    } else if matches!(token, "lfo" | "mod" | "modulation" | "macro") {
        ParamGroup::Mod
    } else if matches!(token, "fx" | "effect" | "delay" | "reverb" | "shaper" | "drive" | "dist" | "chorus") {
        ParamGroup::Fx
    } else if matches!(token, "mix" | "out" | "output" | "master" | "level" | "gain" | "pan" | "volume") {
        ParamGroup::Mix
    } else {
        ParamGroup::Other(token.to_string())
    }
}

fn glyph_schema_for_descriptor(
    descriptor: &sequencer::effects::EffectDescriptor,
    id_prefix: &str,
    order_offset: usize,
    forced_group: Option<ParamGroup>,
) -> Vec<ParamSchema> {
    descriptor.params.iter().enumerate().map(|(order, param)| {
        let metadata = param.ui_metadata.as_ref();
        let modulation_plumbing = param.name.starts_with("__dgen_mod_active__")
            || (param.name.starts_with("mod ") && param.name.contains(" slot ") && param.name.ends_with(" amt"));
        let hidden = modulation_plumbing || metadata.is_some_and(|metadata| {
            metadata.tags.iter().any(|tag| matches!(tag.as_str(), "hidden" | "ui" | "non-audio"))
        });
        let (kind, taper) = match &param.kind {
            ParamKind::Continuous { unit } => {
                let inferred_log = unit.as_deref().is_some_and(|unit| {
                    matches!(unit.to_ascii_lowercase().as_str(), "hz" | "khz" | "ms" | "s")
                }) && param.min > 0.0 && param.max / param.min >= 50.0;
                (
                    GlyphParamKind::Continuous,
                    match param.scaling {
                        ParamScaling::Exponential => ParamTaper::Exponential(2.0),
                        ParamScaling::Linear if inferred_log => ParamTaper::Log,
                        ParamScaling::Linear => ParamTaper::Linear,
                    },
                )
            }
            ParamKind::Boolean => (GlyphParamKind::Boolean, ParamTaper::Stepped(2)),
            ParamKind::Enum { labels } => (GlyphParamKind::Discrete, ParamTaper::Stepped(labels.len().max(2) as u32)),
        };
        ParamSchema {
            id: format!("{id_prefix}{}", param.name),
            kind,
            range: (param.min, param.max),
            taper,
            group: forced_group.clone().unwrap_or_else(|| {
                glyph_group(metadata.and_then(|metadata| metadata.group.as_deref()), &param.name)
            }),
            order: order_offset + order,
            link: metadata.and_then(|metadata| {
                metadata.env.as_ref().map(|env| format!("env:{env}"))
                    .or_else(|| metadata.role.as_ref().map(|role| {
                        format!("role:{}:{role}", metadata.group.as_deref().unwrap_or("other"))
                    }))
            }),
            visible: !hidden,
            audio: !hidden,
            default: param.default,
            weight: 1.0,
        }
    }).collect()
}

fn patch_glyph_values(
    patch: &sequencer::sequencer::Patch,
    instrument: &sequencer::effects::EffectDescriptor,
) -> Vec<f32> {
    instrument.params.iter().enumerate().map(|(index, param)| {
        patch.instrument_slot.defaults.get(index).copied().unwrap_or(param.default)
    }).collect()
}

/// Resolve cohort statistics once and publish every palette tile from the
/// same reference/cohort snapshot.
fn sync_glyph_frames(app: &app::App, track: usize, entries: &[PaletteEntry], glyphs: &mut GlyphFrames) {
    let fallback_descriptor;
    let descriptor = match app.graph.instrument_descriptors.get(track) {
        Some(descriptor) => descriptor,
        None => {
            fallback_descriptor = sequencer::effects::EffectDescriptor::builtin_sampler();
            &fallback_descriptor
        }
    };
    let schema = glyph_schema_for_descriptor(descriptor, "instrument:", 0, None);
    let identity = identity_branches(app, track, descriptor, glyphs).to_vec();
    let track_instrument_type = app.graph.track_instrument_types.get(track);
    let mut active = HashSet::new();
    let mut pending = Vec::new();
    app.state.with_project_scenes(|scenes| {
        let Some(pool) = scenes.track_pools.get(track) else { return };
        let compatible = |patch: &sequencer::sequencer::Patch| {
            patch.instrument_slot.defaults.len() == descriptor.params.len()
                && track_instrument_type.is_none_or(|instrument_type| &patch.instrument_type == instrument_type)
        };
        let cohort_entries = entries.iter().filter_map(|entry| {
            let patch = pool.sounds.patches.get(&entry.patch)?;
            compatible(patch).then(|| (entry.patch, patch_glyph_values(patch, descriptor)))
        }).collect::<Vec<_>>();
        let cohort = cohort_entries.iter().map(|(_, values)| values.clone()).collect::<Vec<_>>();
        // Anchor mode (spec §7): the reference is the FIRST patch in palette order,
        // not the selection. Diffing against the selection made every tile change
        // shape on every click, which is disorienting exactly while scanning — and
        // it forced a full cohort re-stat on each selection change.
        let anchor_patch = cohort_entries.first().map(|(patch, _)| *patch);
        let reference = cohort.first().cloned().unwrap_or_default();
        let cohort_model =
            DeltaGlyphCohort::new_with_identity(&schema, &cohort, &reference, &identity);

        let mut cohort_hasher = DefaultHasher::new();
        for descriptor in std::iter::once(descriptor) {
            descriptor.name.hash(&mut cohort_hasher);
            for param in &descriptor.params {
                param.name.hash(&mut cohort_hasher);
                param.min.to_bits().hash(&mut cohort_hasher);
                param.max.to_bits().hash(&mut cohort_hasher);
                param.default.to_bits().hash(&mut cohort_hasher);
                std::mem::discriminant(&param.kind).hash(&mut cohort_hasher);
                std::mem::discriminant(&param.scaling).hash(&mut cohort_hasher);
                if let Some(metadata) = &param.ui_metadata {
                    metadata.group.hash(&mut cohort_hasher);
                    metadata.env.hash(&mut cohort_hasher);
                    metadata.role.hash(&mut cohort_hasher);
                    metadata.tags.hash(&mut cohort_hasher);
                }
            }
        }
        for (patch, values) in &cohort_entries {
            patch.hash(&mut cohort_hasher);
            for value in values { value.to_bits().hash(&mut cohort_hasher); }
        }
        for branch in &identity {
            branch.name.hash(&mut cohort_hasher);
            branch.weight.to_bits().hash(&mut cohort_hasher);
        }
        // The anchor is cohort_entries[0], already hashed above, so nothing extra
        // is needed here — and notably the selection is NOT part of the key.
        let cohort_fingerprint = cohort_hasher.finish();

        for entry in entries {
            let Some(patch) = pool.sounds.patches.get(&entry.patch) else { continue };
            let key = glyph_key(track, entry.patch.0);
            active.insert(key.clone());
            let values = patch_glyph_values(patch, descriptor);
            let is_compatible = compatible(patch);
            let is_anchor = anchor_patch == Some(entry.patch);
            let mut hasher = DefaultHasher::new();
            cohort_fingerprint.hash(&mut hasher);
            entry.patch.hash(&mut hasher);
            is_anchor.hash(&mut hasher);
            is_compatible.hash(&mut hasher);
            for value in &values { value.to_bits().hash(&mut hasher); }
            let fingerprint = hasher.finish();
            if glyphs.published.get(&key) == Some(&fingerprint) { continue; }

            // An incompatible patch is normalized against itself, so it renders as
            // the bare grid plus the incompatible ring; it is also excluded from the
            // cohort statistics so it cannot poison anyone else's scale.
            let delta = if is_compatible {
                cohort_model.build(&values, is_anchor)
            } else {
                cohort_model.build(&reference, false)
            };
            glyphs.revision = glyphs.revision.wrapping_add(1);
            pending.push((key.clone(), SoundGlyphFrame {
                revision: glyphs.revision,
                cols: delta.cols,
                rows: delta.rows,
                substrate: delta.substrate,
                pieces: delta.pieces.into_iter().map(|piece| SoundGlyphPiece {
                    slot: piece.slot,
                    piece: piece.piece,
                    hue: piece.hue,
                    magnitude: piece.magnitude,
                    mirror: piece.mirror,
                    negative: piece.negative,
                }).collect(),
                anchor: delta.anchor,
                incompatible: !is_compatible,
            }));
            glyphs.published.insert(key, fingerprint);
        }
    });
    publish_sound_glyph_frames(pending);
    retain_sound_glyph_frames(&active);
    glyphs.published.retain(|key, _| active.contains(key));
}

fn color_fields(map: &mut HashMap<String, Rc<RefCell<Value>>>, color: Option<u8>) {
    match color
        .map(usize::from)
        .filter(|idx| *idx < SOUND_PALETTE_RGB.len())
    {
        Some(idx) => {
            let [r, g, b] = SOUND_PALETTE_RGB[idx];
            map.insert(
                "color".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
            map.insert(
                "color-r".to_string(),
                Rc::new(RefCell::new(Value::Number(r as f64))),
            );
            map.insert(
                "color-g".to_string(),
                Rc::new(RefCell::new(Value::Number(g as f64))),
            );
            map.insert(
                "color-b".to_string(),
                Rc::new(RefCell::new(Value::Number(b as f64))),
            );
        }
        None => {
            map.insert("color".to_string(), Rc::new(RefCell::new(Value::Nil)));
        }
    }
}

/// The track's instrument display name for the palette header — the same
/// resolution the sidebar uses: samplers have no engine name, racks show the
/// track name, and everything else the engine/custom-instrument name.
fn palette_instrument_name(app: &app::App, track: usize) -> String {
    match app.graph.track_instrument_types.get(track) {
        Some(sequencer::sequencer::InstrumentType::Sampler) => "sampler".to_string(),
        Some(sequencer::sequencer::InstrumentType::Rack) => {
            app.tracks.get(track).cloned().unwrap_or_default()
        }
        _ => current_custom_instrument_name(app, track)
            .or_else(|| app.tracks.get(track).cloned())
            .unwrap_or_default(),
    }
}

fn build_palette_value(
    track: usize,
    target: PaletteTarget,
    instrument_name: &str,
    entries: &[PaletteEntry],
) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    map.insert(
        "instrument-name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_name.to_string()))),
    );
    let (kind, id) = match target {
        PaletteTarget::Take(id) => ("take", Some(id.0)),
        PaletteTarget::Pattern(id) => ("pattern", Some(id.0)),
        PaletteTarget::Cell => ("cell", None),
    };
    map.insert(
        "target-kind".to_string(),
        Rc::new(RefCell::new(Value::String(kind.to_string()))),
    );
    map.insert(
        "target-id".to_string(),
        Rc::new(RefCell::new(match id {
            Some(id) => Value::Number(id as f64),
            None => Value::Nil,
        })),
    );
    let rows = entries
        .iter()
        .map(|entry| {
            let mut row = HashMap::new();
            row.insert(
                "patch-id".to_string(),
                Rc::new(RefCell::new(Value::Number(entry.patch.0 as f64))),
            );
            row.insert(
                "mix-id".to_string(),
                Rc::new(RefCell::new(match entry.mix {
                    Some(id) => Value::Number(id.0 as f64),
                    None => Value::Nil,
                })),
            );
            row.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(entry.name.clone()))),
            );
            row.insert(
                "referents".to_string(),
                Rc::new(RefCell::new(Value::String(entry.referents.clone()))),
            );
            row.insert(
                "referents-short".to_string(),
                Rc::new(RefCell::new(Value::String(entry.referents_short.clone()))),
            );
            row.insert(
                "base".to_string(),
                Rc::new(RefCell::new(Value::Bool(entry.is_base))),
            );
            row.insert(
                "current".to_string(),
                Rc::new(RefCell::new(Value::Bool(entry.is_current))),
            );
            row.insert(
                "glyph-key".to_string(),
                Rc::new(RefCell::new(Value::String(glyph_key(track, entry.patch.0)))),
            );
            row.insert(
                "preset".to_string(),
                Rc::new(RefCell::new(match &entry.preset {
                    Some(name) => Value::String(name.clone()),
                    None => Value::Nil,
                })),
            );
            row.insert(
                "sample".to_string(),
                Rc::new(RefCell::new(match &entry.sample {
                    Some(name) => Value::String(name.clone()),
                    None => Value::Nil,
                })),
            );
            row.insert(
                "diff-up".to_string(),
                Rc::new(RefCell::new(Value::Number(entry.params_up as f64))),
            );
            row.insert(
                "diff-down".to_string(),
                Rc::new(RefCell::new(Value::Number(entry.params_down as f64))),
            );
            color_fields(&mut row, entry.color);
            Rc::new(RefCell::new(Value::Map(row)))
        })
        .collect();
    map.insert(
        "entries".to_string(),
        Rc::new(RefCell::new(Value::List(rows))),
    );
    Value::Map(map)
}

fn build_clip_sounds_value(tracks: &[Vec<(u64, bool, Option<u8>)>]) -> Value {
    Value::List(
        tracks
            .iter()
            .map(|clips| {
                let clips = clips
                    .iter()
                    .map(|(clip_id, dot, color)| {
                        let mut map = HashMap::new();
                        map.insert(
                            "clip-id".to_string(),
                            Rc::new(RefCell::new(Value::Number(*clip_id as f64))),
                        );
                        map.insert("dot".to_string(), Rc::new(RefCell::new(Value::Bool(*dot))));
                        color_fields(&mut map, *color);
                        Rc::new(RefCell::new(Value::Map(map)))
                    })
                    .collect();
                Rc::new(RefCell::new(Value::List(clips)))
            })
            .collect(),
    )
}

/// Publish the palette read surfaces. Returns true when a reactive cycle is
/// needed. The clip-sounds join (two lock scopes + a full per-clip build)
/// only runs while the arrangement is visible — nothing else reads it; the
/// cache clears on hide so re-showing republishes fresh. The palette half
/// stays ungated (cheap, and it also mounts in the *step* side panel).
pub(crate) fn sync_sound_palette(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SoundPaletteFrameState,
    arrangement_visible: bool,
) -> bool {
    let mut dirty = false;
    match app.sound_palette_open {
        Some((track, target)) => {
            let entries = app.sound_palette_entries(track, target);
            sync_glyph_frames(app, track, &entries, &mut frame.glyphs);
            let snapshot = (track, target, palette_instrument_name(app, track), entries);
            if frame.cached.as_ref() != Some(&snapshot) {
                dirty |= rt
                    .set_reactive(
                        "SEQ",
                        "sound-palette",
                        build_palette_value(snapshot.0, snapshot.1, &snapshot.2, &snapshot.3),
                    )
                    .effects_dirty;
                frame.cached = Some(snapshot);
                frame.published_open = true;
            }
        }
        None => {
            if frame.published_open || frame.cached.is_some() {
                dirty |= rt
                    .set_reactive("SEQ", "sound-palette", Value::Nil)
                    .effects_dirty;
                frame.cached = None;
                frame.published_open = false;
                retain_sound_glyph_frames(&HashSet::new());
                frame.glyphs.published.clear();
            }
        }
    }
    if arrangement_visible {
        let clip_sounds = app.song_clip_sounds();
        if frame.cached_clip_sounds.as_ref() != Some(&clip_sounds) {
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    "song-clip-sounds",
                    build_clip_sounds_value(&clip_sounds),
                )
                .effects_dirty;
            frame.cached_clip_sounds = Some(clip_sounds);
        }
    } else {
        // Hidden: skip the join entirely and forget the cache so the next
        // visible frame recomputes and republishes.
        frame.cached_clip_sounds = None;
    }
    dirty
}
