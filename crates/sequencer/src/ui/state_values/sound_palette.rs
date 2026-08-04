//! Sound palette read surfaces (takes spec §17.6 / §18.3):
//! `SEQ.sound-palette` (the open overlay's entries) and
//! `SEQ.song-clip-sounds` (the timeline clip-dot identity join). Both diff by value
//! before publishing, like `scene-names` — the underlying scenes have no
//! revision counter and palette gestures can move refs without touching the
//! committed-song revision.

use super::*;
use crate::app::sound_palette::{PaletteEntry, PaletteTarget, SOUND_PALETTE_RGB};
use eseqlisp::sound_glyph_data::{
    publish_sound_glyph_frames, retain_sound_glyph_frames, set_sound_glyph_play_keys,
    SoundGlyphFrame, SoundGlyphPiece,
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
    /// cached per track because resolving them reads and parses the
    /// instrument's dsp.lisp (~0.5–0.7ms per extraction).
    identity: HashMap<usize, IdentityCacheEntry>,
    /// Per-track fingerprints for the mixer pattern-cell glyph feed, which
    /// runs every sync regardless of whether the palette is open.
    cell_published: HashMap<usize, u64>,
    /// Per-track cache of `hash_descriptor_glyph_inputs` (SipHash over every
    /// param's name/range/kind/taper/ui metadata — ~110ms/s across a scroll
    /// profile when recomputed per tick). See `cached_descriptor_glyph_hash`
    /// for why the probe is sufficient.
    descriptor_hashes: HashMap<usize, DescriptorGlyphHashEntry>,
}

struct DescriptorGlyphHashEntry {
    engine_id: Option<usize>,
    registry_epoch: u64,
    name: String,
    param_count: usize,
    hash: u64,
}

struct IdentityCacheEntry {
    cache_key: String,
    branches: Vec<IdentityBranch>,
    /// Hash of the branches, precomputed at fill so the per-frame dirty
    /// check never re-hashes (or clones) up to ~76 branch names.
    fingerprint: u64,
}

/// FNV-1a over a value vector: the per-frame dirty checks fold each patch's
/// ~110 floats into one u64 before it reaches the (much slower per-write)
/// SipHash fingerprint hasher.
fn values_fingerprint(values: impl IntoIterator<Item = f32>) -> u64 {
    values.into_iter().fold(0xcbf29ce484222325u64, |hash, value| {
        (hash ^ value.to_bits() as u64).wrapping_mul(0x100000001b3)
    })
}

/// Cache key for the identity tier: instrument name, descriptor param count,
/// the track's engine id, and the engine-registry epoch. The epoch bumps on
/// every compile/reload registration, so any reload — including ones that
/// restructure branch clusters without touching the param surface — changes
/// the key and refreshes the cached silhouette, without touching the source
/// text (or the filesystem) on the per-tick probe.
fn identity_cache_key(
    custom: Option<&str>,
    param_count: usize,
    engine_id: Option<usize>,
    registry_epoch: u64,
) -> String {
    format!(
        "{}:{}:{}:{}",
        custom.unwrap_or("stock"),
        param_count,
        engine_id.map_or(-1i64, |id| id as i64),
        registry_epoch
    )
}

/// The identity tier's input: branch clusters of the instrument's authored AST,
/// via `extract_skeleton` for custom instruments and the stock skeleton for
/// builtins/samplers. Cached on the instrument name + descriptor param count +
/// engine id + registry epoch, so any instrument reload — even one that
/// leaves the param surface unchanged — refreshes it (a reload re-registers
/// the engine, which bumps the epoch). The per-call freshness probe is a few
/// in-memory reads; the ~0.5–0.7ms skeleton extraction — and, for tracks
/// whose engine isn't registered, a disk read of the dsp.lisp — only run on
/// a key miss. The glyph deliberately mirrors what's COMPILED, not what's on
/// disk: an external dsp.lisp edit doesn't change the sound (or the
/// silhouette) until it's reloaded through the app.
fn ensure_identity_cached<'a>(
    app: &app::App,
    track: usize,
    descriptor: &sequencer::effects::EffectDescriptor,
    cache: &'a mut HashMap<usize, IdentityCacheEntry>,
) -> &'a IdentityCacheEntry {
    let custom = current_custom_instrument_name(app, track);
    let engine_id = app
        .graph
        .track_engine_ids
        .get(track)
        .and_then(|engine_id| *engine_id);
    let cache_key = identity_cache_key(
        custom.as_deref(),
        descriptor.params.len(),
        engine_id,
        app.editor.engine_registry.epoch(),
    );
    if cache.get(&track).map(|entry| entry.cache_key.as_str()) != Some(cache_key.as_str()) {
        let source = engine_id
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| engine.source.clone())
            .or_else(|| {
                custom
                    .as_deref()
                    .and_then(|name| sequencer::lisp_host::load_instrument_source(name).ok())
            });
        let skeleton = source
            .map(|source| sequencer::sound_glyph::extract_skeleton(&source).skeleton)
            .unwrap_or_else(|| sequencer::sound_glyph::stock_skeleton(descriptor).skeleton);
        let branches = sequencer::sound_glyph::identity_branches(&skeleton);
        let mut hasher = DefaultHasher::new();
        for branch in &branches {
            branch.name.hash(&mut hasher);
            branch.weight.to_bits().hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        cache.insert(track, IdentityCacheEntry { cache_key, branches, fingerprint });
    }
    cache.get(&track).expect("just inserted")
}

/// Hash every descriptor field that feeds `glyph_schema_for_descriptor`.
/// Both glyph feeds — the palette tiles and the mixer pattern cells — MUST
/// fingerprint the descriptor with this one helper (via
/// `cached_descriptor_glyph_hash`): an instrument reload that
/// keeps the param names/count but changes a range, default, kind, taper, or
/// ui metadata reshapes the glyph schema, so it has to invalidate both feeds
/// identically.
fn hash_descriptor_glyph_inputs(
    descriptor: &sequencer::effects::EffectDescriptor,
    hasher: &mut impl Hasher,
) {
    descriptor.name.hash(hasher);
    descriptor.params.len().hash(hasher);
    for param in &descriptor.params {
        param.name.hash(hasher);
        param.min.to_bits().hash(hasher);
        param.max.to_bits().hash(hasher);
        param.default.to_bits().hash(hasher);
        std::mem::discriminant(&param.kind).hash(hasher);
        std::mem::discriminant(&param.scaling).hash(hasher);
        if let Some(metadata) = &param.ui_metadata {
            metadata.group.hash(hasher);
            metadata.env.hash(hasher);
            metadata.role.hash(hasher);
            metadata.tags.hash(hasher);
        }
    }
}

/// `hash_descriptor_glyph_inputs`, memoized per track. The probe —
/// (engine id, registry epoch, descriptor name, param count) — is sufficient
/// because every path that reshapes a descriptor's glyph inputs either
/// recompiles/re-registers an engine (registry epoch bump), rebinds the track
/// to a different engine (engine id change), or swaps the descriptor for one
/// with a different name/param surface (teardown to the empty slot, sampler
/// fallback). A descriptor NEVER mutates in place with all four probes
/// unchanged.
fn cached_descriptor_glyph_hash(
    app: &app::App,
    track: usize,
    descriptor: &sequencer::effects::EffectDescriptor,
    cache: &mut HashMap<usize, DescriptorGlyphHashEntry>,
) -> u64 {
    let engine_id = app
        .graph
        .track_engine_ids
        .get(track)
        .and_then(|engine_id| *engine_id);
    let registry_epoch = app.editor.engine_registry.epoch();
    if let Some(entry) = cache.get(&track) {
        if entry.engine_id == engine_id
            && entry.registry_epoch == registry_epoch
            && entry.name == descriptor.name
            && entry.param_count == descriptor.params.len()
        {
            return entry.hash;
        }
    }
    let mut hasher = DefaultHasher::new();
    hash_descriptor_glyph_inputs(descriptor, &mut hasher);
    let hash = hasher.finish();
    cache.insert(
        track,
        DescriptorGlyphHashEntry {
            engine_id,
            registry_epoch,
            name: descriptor.name.clone(),
            param_count: descriptor.params.len(),
            hash,
        },
    );
    hash
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
    let descriptor_hash =
        cached_descriptor_glyph_hash(app, track, descriptor, &mut glyphs.descriptor_hashes);
    let identity_entry = ensure_identity_cached(app, track, descriptor, &mut glyphs.identity);
    let identity_fingerprint = identity_entry.fingerprint;
    // Disjoint reborrows so the cached branches stay borrowed while the
    // closure below writes the sibling fields.
    let identity = &identity_entry.branches;
    let published = &mut glyphs.published;
    let revision = &mut glyphs.revision;
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
        // Anchor mode (spec §7): the reference is the FIRST patch in palette order,
        // not the selection. Diffing against the selection made every tile change
        // shape on every click, which is disorienting exactly while scanning — and
        // it forced a full cohort re-stat on each selection change.
        let anchor_patch = cohort_entries.first().map(|(patch, _)| *patch);

        let mut cohort_hasher = DefaultHasher::new();
        descriptor_hash.hash(&mut cohort_hasher);
        for (patch, values) in &cohort_entries {
            patch.hash(&mut cohort_hasher);
            values_fingerprint(values.iter().copied()).hash(&mut cohort_hasher);
        }
        identity_fingerprint.hash(&mut cohort_hasher);
        // The anchor is cohort_entries[0], already hashed above, so nothing extra
        // is needed here — and notably the selection is NOT part of the key.
        let cohort_fingerprint = cohort_hasher.finish();

        // Pass 1: fingerprint every tile and collect the misses, so the
        // steady state (palette open, nothing changing) never constructs the
        // schema or re-runs the cohort statistics.
        struct Miss {
            key: String,
            fingerprint: u64,
            values: Vec<f32>,
            is_anchor: bool,
            is_compatible: bool,
        }
        let mut misses = Vec::new();
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
            values_fingerprint(values.iter().copied()).hash(&mut hasher);
            let fingerprint = hasher.finish();
            if published.get(&key) == Some(&fingerprint) { continue; }
            misses.push(Miss { key, fingerprint, values, is_anchor, is_compatible });
        }
        if misses.is_empty() {
            return;
        }

        let schema = glyph_schema_for_descriptor(descriptor, "instrument:", 0, None);
        let cohort = cohort_entries.iter().map(|(_, values)| values.clone()).collect::<Vec<_>>();
        let reference = cohort.first().cloned().unwrap_or_default();
        let cohort_model =
            DeltaGlyphCohort::new_with_identity(&schema, &cohort, &reference, identity);
        for miss in misses {
            // An incompatible patch is normalized against itself, so it renders as
            // the bare grid plus the incompatible ring; it is also excluded from the
            // cohort statistics so it cannot poison anyone else's scale.
            let delta = if miss.is_compatible {
                cohort_model.build(&miss.values, miss.is_anchor)
            } else {
                cohort_model.build(&reference, false)
            };
            *revision = revision.wrapping_add(1);
            pending.push((miss.key.clone(), SoundGlyphFrame {
                revision: *revision,
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
                incompatible: !miss.is_compatible,
            }));
            published.insert(miss.key, miss.fingerprint);
        }
    });
    publish_sound_glyph_frames(pending);
    retain_sound_glyph_frames("sound-glyph:", &active);
    glyphs.published.retain(|key, _| active.contains(key));
}

fn pattern_cell_glyph_key(track: usize, pattern: u64) -> String {
    format!("pattern-glyph:track:{track}:pattern:{pattern}")
}

/// Publish one glyph frame per mixer pattern-launch cell, keyed on
/// `(track, pattern)` so the lisp side needs no extra plumbing. Cohort = the
/// distinct patches the track's cells reference; the reference is the
/// track's *effective* sound, so cells bound to what is currently sounding
/// render as substrate-only (plus the anchor ring) and diverged cells grow
/// accent pieces. Runs every sync — the per-track fingerprint keeps the
/// steady-state cost at hashing a few value vectors.
fn sync_pattern_cell_glyph_frames(app: &app::App, glyphs: &mut GlyphFrames) {
    let mut active: HashSet<String> = HashSet::new();
    let mut play_keys: HashSet<String> = HashSet::new();
    let mut pending = Vec::new();
    let mut tracks_seen: HashSet<usize> = HashSet::new();
    // One fallback for the whole sweep — building a builtin-sampler
    // descriptor per sampler track per frame is pure allocation churn.
    let fallback_descriptor = sequencer::effects::EffectDescriptor::builtin_sampler();
    // Disjoint field reborrows: the identity cache is read (borrowed) per
    // track while the closure writes the sibling bookkeeping fields.
    let GlyphFrames {
        identity: identity_cache,
        cell_published,
        revision,
        descriptor_hashes,
        ..
    } = glyphs;
    for track in 0..app.tracks.len() {
        let cells = app.state.track_pattern_cells(track);
        if cells.is_empty() {
            continue;
        }
        tracks_seen.insert(track);
        // Launch state feeds the glyph shader's play triangle. It lives in a
        // side store (not the frame): a launch change must invalidate widget
        // primitives, but it must NOT force a cohort re-stat.
        for cell in &cells {
            if cell.active_effective {
                play_keys.insert(pattern_cell_glyph_key(track, cell.pattern_id.0));
            }
        }
        let descriptor = app
            .graph
            .instrument_descriptors
            .get(track)
            .unwrap_or(&fallback_descriptor);
        let descriptor_hash =
            cached_descriptor_glyph_hash(app, track, descriptor, descriptor_hashes);
        let identity_entry = ensure_identity_cached(app, track, descriptor, identity_cache);
        let identity_fingerprint = identity_entry.fingerprint;
        let identity = &identity_entry.branches;
        let track_instrument_type = app.graph.track_instrument_types.get(track);
        app.state.with_project_scenes(|scenes| {
            let Some(pool) = scenes.track_pools.get(track) else { return };
            let compatible = |patch: &sequencer::sequencer::Patch| {
                patch.instrument_slot.defaults.len() == descriptor.params.len()
                    && track_instrument_type
                        .is_none_or(|instrument_type| &patch.instrument_type == instrument_type)
            };
            // Reference first, then each cell's patch in cell order, deduped.
            // The reference must be SCENE-INDEPENDENT (spec §7 anchor mode):
            // diffing against the effective sound re-shapes every cell on each
            // scene switch. The pool's lowest patch id is stable — glyphs only
            // change when parameters or the pool itself change.
            let reference_patch = pool.sounds.patches.keys().min().copied();
            let cell_patches = cells
                .iter()
                .map(|cell| (cell.pattern_id, pool.refs(cell.pattern_id).map(|refs| refs.patch)))
                .collect::<Vec<_>>();
            let mut patches = reference_patch.into_iter().collect::<Vec<_>>();
            for (_, patch) in &cell_patches {
                if let Some(patch) = patch {
                    if !patches.contains(patch) {
                        patches.push(*patch);
                    }
                }
            }
            for (pattern, _) in &cell_patches {
                active.insert(pattern_cell_glyph_key(track, pattern.0));
            }

            let mut hasher = DefaultHasher::new();
            descriptor_hash.hash(&mut hasher);
            identity_fingerprint.hash(&mut hasher);
            for (pattern, patch) in &cell_patches {
                pattern.hash(&mut hasher);
                patch.hash(&mut hasher);
            }
            for patch in &patches {
                patch.hash(&mut hasher);
                if let Some(patch) = pool.sounds.patches.get(patch) {
                    values_fingerprint(patch.instrument_slot.defaults.iter().copied())
                        .hash(&mut hasher);
                    compatible(patch).hash(&mut hasher);
                }
            }
            let fingerprint = hasher.finish();
            if cell_published.get(&track) == Some(&fingerprint) {
                return;
            }
            cell_published.insert(track, fingerprint);

            let schema = glyph_schema_for_descriptor(descriptor, "instrument:", 0, None);
            let cohort = patches
                .iter()
                .filter_map(|id| {
                    let patch = pool.sounds.patches.get(id)?;
                    compatible(patch).then(|| (*id, patch_glyph_values(patch, descriptor)))
                })
                .collect::<Vec<_>>();
            let values = cohort.iter().map(|(_, values)| values.clone()).collect::<Vec<_>>();
            let reference = values.first().cloned().unwrap_or_default();
            let model = DeltaGlyphCohort::new_with_identity(&schema, &values, &reference, identity);
            for (pattern, patch) in &cell_patches {
                // A cell whose pattern has no bound patch still publishes an
                // (empty-substrate) frame: the glyph widget now owns the play
                // indicator, and a missing frame would drop the primitive
                // entirely — leaving an active-but-unbound cell with no
                // triangle at all.
                let Some(patch) = patch else {
                    *revision = revision.wrapping_add(1);
                    pending.push((
                        pattern_cell_glyph_key(track, pattern.0),
                        SoundGlyphFrame {
                            revision: *revision,
                            cols: 1,
                            rows: 1,
                            substrate: vec![0],
                            pieces: Vec::new(),
                            anchor: false,
                            incompatible: false,
                        },
                    ));
                    continue;
                };
                // anchor=false everywhere: the palette's anchor ring marks the
                // reference tile, which in the cell grid is an arbitrary pool
                // patch — the launch state already has its own indicators.
                let built = match cohort.iter().position(|(id, _)| id == patch) {
                    Some(index) => model.build(&values[index], false),
                    // Incompatible patch: normalized against the reference and
                    // ringed, exactly as the palette does.
                    None => {
                        let mut incompatible = model.build(&reference, false);
                        incompatible.pieces.clear();
                        incompatible
                    }
                };
                *revision = revision.wrapping_add(1);
                pending.push((
                    pattern_cell_glyph_key(track, pattern.0),
                    SoundGlyphFrame {
                        revision: *revision,
                        cols: built.cols,
                        rows: built.rows,
                        substrate: built.substrate,
                        pieces: built
                            .pieces
                            .into_iter()
                            .map(|piece| SoundGlyphPiece {
                                slot: piece.slot,
                                piece: piece.piece,
                                hue: piece.hue,
                                magnitude: piece.magnitude,
                                mirror: piece.mirror,
                                negative: piece.negative,
                            })
                            .collect(),
                        anchor: built.anchor,
                        incompatible: !cohort.iter().any(|(id, _)| id == patch),
                    },
                ));
            }
        });
    }
    if !pending.is_empty() {
        publish_sound_glyph_frames(pending);
    }
    retain_sound_glyph_frames("pattern-glyph:", &active);
    set_sound_glyph_play_keys("pattern-glyph:", play_keys);
    cell_published.retain(|track, _| tracks_seen.contains(track));
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
    sync_pattern_cell_glyph_frames(app, &mut frame.glyphs);
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
                retain_sound_glyph_frames("sound-glyph:", &HashSet::new());
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

#[cfg(test)]
mod glyph_fingerprint_tests {
    use super::*;
    use sequencer::effects::{EffectDescriptor, ParamDescriptor, ParamUiMetadata};

    fn param(name: &str) -> ParamDescriptor {
        ParamDescriptor {
            name: name.to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: 0,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        }
    }

    fn descriptor(params: Vec<ParamDescriptor>) -> EffectDescriptor {
        EffectDescriptor {
            name: "test-instrument".to_string(),
            params,
            tensor_params: Vec::new(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
        }
    }

    fn fingerprint(descriptor: &EffectDescriptor) -> u64 {
        let mut hasher = DefaultHasher::new();
        hash_descriptor_glyph_inputs(descriptor, &mut hasher);
        hasher.finish()
    }

    /// The mixer-cell feed once hashed only name + param count; a reload that
    /// kept both but changed a param's range/taper/metadata left its glyphs
    /// stale. The shared helper must react to every schema input.
    #[test]
    fn descriptor_fingerprint_tracks_every_schema_input() {
        let base = descriptor(vec![param("cutoff"), param("res")]);
        assert_eq!(
            fingerprint(&base),
            fingerprint(&descriptor(vec![param("cutoff"), param("res")]))
        );

        let mut range = descriptor(vec![param("cutoff"), param("res")]);
        range.params[0].max = 2.0;
        assert_ne!(fingerprint(&base), fingerprint(&range));

        let mut default = descriptor(vec![param("cutoff"), param("res")]);
        default.params[1].default = 0.25;
        assert_ne!(fingerprint(&base), fingerprint(&default));

        let mut taper = descriptor(vec![param("cutoff"), param("res")]);
        taper.params[0].scaling = ParamScaling::Exponential;
        assert_ne!(fingerprint(&base), fingerprint(&taper));

        let mut kind = descriptor(vec![param("cutoff"), param("res")]);
        kind.params[1].kind = ParamKind::Boolean;
        assert_ne!(fingerprint(&base), fingerprint(&kind));

        let mut metadata = descriptor(vec![param("cutoff"), param("res")]);
        metadata.params[0].ui_metadata =
            ParamUiMetadata::new(Some("filter".to_string()), None, None);
        assert_ne!(fingerprint(&base), fingerprint(&metadata));
    }

    /// Same engine + registry epoch, same key — the ~0.5-0.7ms skeleton
    /// extraction stays cached across frames.
    #[test]
    fn identity_cache_key_stable_for_unchanged_engine() {
        assert_eq!(
            identity_cache_key(Some("my-synth"), 8, Some(3), 7),
            identity_cache_key(Some("my-synth"), 8, Some(3), 7)
        );
    }

    /// An instrument reload that restructures branch clusters WITHOUT changing
    /// the param surface (same name, same param count, same engine slot) still
    /// bumps the registry epoch and must change the key, so the identity
    /// silhouette refreshes.
    #[test]
    fn identity_cache_key_changes_with_registry_epoch() {
        let before = identity_cache_key(Some("my-synth"), 8, Some(3), 7);
        let after = identity_cache_key(Some("my-synth"), 8, Some(3), 8);
        assert_ne!(before, after);
    }

    /// Rebinding the track to a different registered engine changes the key
    /// even when the epoch hasn't moved (swap-instrument reuses compiled
    /// engines without re-registering).
    #[test]
    fn identity_cache_key_changes_with_engine_id() {
        let before = identity_cache_key(Some("my-synth"), 8, Some(3), 7);
        let after = identity_cache_key(Some("my-synth"), 8, Some(4), 7);
        assert_ne!(before, after);
    }

    /// Builtins/samplers have no engine; the stock key still discriminates on
    /// name and param count.
    #[test]
    fn identity_cache_key_stock_paths_differ_by_surface() {
        assert_eq!(
            identity_cache_key(None, 8, None, 0),
            identity_cache_key(None, 8, None, 0)
        );
        assert_ne!(
            identity_cache_key(None, 8, None, 0),
            identity_cache_key(None, 9, None, 0)
        );
        assert_ne!(
            identity_cache_key(None, 8, None, 0),
            identity_cache_key(Some("my-synth"), 8, None, 0)
        );
    }
}
