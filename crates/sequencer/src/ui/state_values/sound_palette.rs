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
pub(super) struct GlyphFrames {
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
    /// Rack tracks only (rack-glyph spec §2.4): the rack glyph signature the
    /// hash was computed against. `None` for flat tracks, which keeps the
    /// flat probe bit-for-bit what it was. Racks need it because a rack edit
    /// — slot added, slot's engine swapped, macro count changed — moves
    /// neither the track engine id nor the (always `empty_custom_slot`)
    /// descriptor's name/param count, so the four flat probes can never fire.
    rack_signature: Option<u64>,
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
    fold_values(0xcbf29ce484222325u64, values)
}

/// `values_fingerprint` with an explicit seed, so a composite (a rack's
/// per-slot vectors plus its macros) folds in one pass without materializing
/// the concatenation.
fn fold_values(seed: u64, values: impl IntoIterator<Item = f32>) -> u64 {
    values.into_iter().fold(seed, |hash, value| {
        (hash ^ value.to_bits() as u64).wrapping_mul(0x100000001b3)
    })
}

// ── rack composite surface (docs/rack-glyph-spec.md) ──────────────────────
//
// A rack track's `instrument_descriptors[track]` is `empty_custom_slot()`
// (`finish_rack_track_registration`), so the flat path yields an empty schema,
// an empty skeleton and empty values — three independent collapses that all
// render as a blank glyph. The composite surface below rebuilds the same three
// inputs out of the rack snapshot instead, one spatial lobe per slot.

/// Weight (`ParamSchema::weight`, 1.0 everywhere else) for a rack macro. The
/// macros are the rack's *played* surface; at equal normalized deviation a
/// macro must win piece selection against some slot's deep oscillator param.
const RACK_MACRO_WEIGHT: f32 = 1.75;

/// The one builtin-sampler descriptor for the whole process. It is immutable
/// and identical for every track, and BOTH feeds need it on every sync — as
/// the flat fallback for a track with no registered descriptor and as the
/// resolved descriptor of every sampler rack slot. Building it per sync (per
/// *track*, before the feeds hoisted it) is pure allocation churn; a
/// process-lifetime borrow also lets `GlyphSurface` hold it without tying its
/// lifetime to a caller's stack slot.
fn fallback_sampler_descriptor() -> &'static sequencer::effects::EffectDescriptor {
    static SAMPLER: std::sync::OnceLock<sequencer::effects::EffectDescriptor> =
        std::sync::OnceLock::new();
    SAMPLER.get_or_init(sequencer::effects::EffectDescriptor::builtin_sampler)
}

/// The rack fields the glyph reads, projected off the (large)
/// `RackTrackSnapshot`. The mixer-cell feed runs every sync for **every**
/// track, and cloning the real snapshot drags each slot's p-lock grid, fx
/// descriptor chain and effect slots along with it — the same
/// disproportionate-clone problem `rack_slot_instrument_param_route` avoids on
/// the control path. The glyph only needs each slot's binding, its instrument
/// defaults, its mix scalars and the macro values.
#[derive(Clone, Debug, PartialEq)]
struct RackGlyphSnapshot {
    slots: Vec<RackSlotGlyph>,
    macros: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
struct RackSlotGlyph {
    instrument_type: sequencer::sequencer::InstrumentType,
    engine_id: Option<usize>,
    defaults: Vec<f32>,
    gain: f32,
    pan: f32,
    mute: bool,
}

fn project_rack_glyph(rack: &sequencer::sequencer::RackTrackSnapshot) -> RackGlyphSnapshot {
    RackGlyphSnapshot {
        slots: rack
            .slots
            .iter()
            .map(|slot| RackSlotGlyph {
                instrument_type: slot.instrument_type,
                engine_id: slot.track_sound_state.engine_id,
                defaults: slot.instrument_slot.defaults.clone(),
                gain: slot.gain,
                pan: slot.pan,
                mute: slot.mute,
            })
            .collect(),
        macros: rack.macros.iter().map(|entry| entry.value).collect(),
    }
}

/// The track's LIVE rack, read through the same accessor the rack UI uses
/// (`state.pattern.rack_tracks`, cf. `select-rack-slot`). Deliberately NOT the
/// pool entity or disk: the glyph mirrors what is COMPILED, and the pool patch
/// is exactly the thing being diffed against this. The lock is taken only long
/// enough to project — never across `with_project_scenes`, which the feeds
/// enter afterwards.
fn live_rack_glyph_snapshot(app: &app::App, track: usize) -> Option<RackGlyphSnapshot> {
    if app.graph.track_instrument_types.get(track)
        != Some(&sequencer::sequencer::InstrumentType::Rack)
    {
        return None;
    }
    let racks = app.state.pattern.rack_tracks.lock().unwrap();
    racks.get(track)?.as_ref().map(project_rack_glyph)
}

/// Resolve a slot's instrument descriptor. Mirrors
/// `App::rack_slot_instrument_descriptor` (`src/app/synth.rs`) but over the
/// projection above and **without cloning** the descriptor: this runs per slot
/// per rack track per sync, and the registry already owns a stable copy. A
/// nested `InstrumentType::Rack` slot resolves nothing and contributes only
/// its mix params (rack-glyph spec §5).
fn rack_slot_binding_descriptor(
    app: &app::App,
    instrument_type: sequencer::sequencer::InstrumentType,
    engine_id: Option<usize>,
) -> Option<&sequencer::effects::EffectDescriptor> {
    match instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => Some(fallback_sampler_descriptor()),
        sequencer::sequencer::InstrumentType::Custom
        | sequencer::sequencer::InstrumentType::Modulator => {
            engine_id.and_then(|engine_id| {
                app.editor.engine_registry.get_instrument_descriptor(engine_id)
            })
        }
        sequencer::sequencer::InstrumentType::Rack => None,
    }
}

fn rack_slot_glyph_descriptor<'a>(
    app: &'a app::App,
    slot: &RackSlotGlyph,
) -> Option<&'a sequencer::effects::EffectDescriptor> {
    rack_slot_binding_descriptor(app, slot.instrument_type, slot.engine_id)
}

/// Synthesized `ParamSchema` for one of a slot's three mix scalars. These are
/// not descriptor params — they live on `RackSlotSnapshot` — but they are what
/// players actually move between two patches of the same layered rack, so they
/// have to diff like any other parameter.
fn rack_mix_schema(
    id: String,
    range: (f32, f32),
    default: f32,
    kind: GlyphParamKind,
    taper: ParamTaper,
    group: ParamGroup,
    order: usize,
) -> ParamSchema {
    ParamSchema {
        id,
        kind,
        range,
        taper,
        group,
        order,
        link: None,
        visible: true,
        audio: true,
        default,
        weight: 1.0,
    }
}

/// Composite rack schema (spec §2.1): one forced `ParamGroup::Other("slot{i}")`
/// lobe per slot — the slot's whole instrument surface plus its gain/pan/mute —
/// then the rack macros as the played surface. Forcing one group per slot is
/// the entire point: interleaving three instruments' osc/filter/env groups
/// produces indistinguishable mush and saturates `MAX_LIT` on nearly every
/// diff, whereas one lobe per slot keeps localization meaningful and lets the
/// existing top-K piece selection spread across lobes.
fn rack_glyph_schema(app: &app::App, rack: &RackGlyphSnapshot) -> Vec<ParamSchema> {
    let mut schema: Vec<ParamSchema> = Vec::new();
    for (index, slot) in rack.slots.iter().enumerate() {
        let group = ParamGroup::Other(format!("slot{index}"));
        let prefix = format!("slot{index}:");
        // A slot whose engine isn't registered contributes only its mix
        // params: the schema must never block on a disk load.
        if let Some(descriptor) = rack_slot_glyph_descriptor(app, slot) {
            let params =
                glyph_schema_for_descriptor(descriptor, &prefix, schema.len(), Some(group.clone()));
            schema.extend(params);
        }
        schema.push(rack_mix_schema(
            format!("{prefix}gain"),
            (0.0, 2.0),
            1.0,
            GlyphParamKind::Continuous,
            ParamTaper::Linear,
            group.clone(),
            schema.len(),
        ));
        schema.push(rack_mix_schema(
            format!("{prefix}pan"),
            (-1.0, 1.0),
            0.0,
            GlyphParamKind::Continuous,
            ParamTaper::Linear,
            group.clone(),
            schema.len(),
        ));
        schema.push(rack_mix_schema(
            format!("{prefix}mute"),
            (0.0, 1.0),
            0.0,
            GlyphParamKind::Boolean,
            ParamTaper::Stepped(2),
            group,
            schema.len(),
        ));
    }
    for index in 0..rack.macros.len() {
        schema.push(ParamSchema {
            id: format!("macro:{index}"),
            kind: GlyphParamKind::Continuous,
            range: (0.0, 1.0),
            taper: ParamTaper::Linear,
            group: ParamGroup::Mod,
            order: schema.len(),
            link: None,
            visible: true,
            audio: true,
            default: 0.0,
            weight: RACK_MACRO_WEIGHT,
        });
    }
    schema
}

/// Composite rack values (spec §2.2), walking the same loop as
/// `rack_glyph_schema` so the two orderings cannot drift: per slot the
/// instrument defaults (padded from the descriptor exactly like
/// `patch_glyph_values`), then gain/pan/mute, then the macro values.
fn rack_glyph_values(app: &app::App, rack: &RackGlyphSnapshot) -> Vec<f32> {
    let mut values = Vec::new();
    for slot in &rack.slots {
        if let Some(descriptor) = rack_slot_glyph_descriptor(app, slot) {
            values.extend(descriptor.params.iter().enumerate().map(|(index, param)| {
                slot.defaults.get(index).copied().unwrap_or(param.default)
            }));
        }
        values.push(slot.gain);
        values.push(slot.pan);
        values.push(if slot.mute { 1.0 } else { 0.0 });
    }
    values.extend(rack.macros.iter().copied());
    values
}

/// The rack's **glyph** signature: everything that reshapes the composite
/// schema (slot count, per-slot binding, per-slot descriptor param count,
/// macro count) and nothing that doesn't. Deliberately looser than
/// `rack_topology_signature` (`src/app/graph/mod.rs`), which folds in fx-chain
/// node ids that churn on every graph rebuild without moving one pixel of the
/// glyph. Doubles as the compatibility yardstick (spec §2.4): a pool patch
/// whose rack signature differs from the live rack's is structurally
/// incompatible, the same way a flat track's patch is when the instrument was
/// swapped underneath it.
///
/// It reads ONLY slot bindings and two counts, so it is computed straight off
/// whichever rack shape is at hand — never through `project_rack_glyph`. That
/// matters: `compatible()` runs per pool patch per sync, and projecting a
/// patch's rack just to hash its lineup would clone every slot's defaults
/// vector (a 16-pad rack with 6 patches is ~6k allocations/second at 60Hz).
///
/// Note `macros.len()` is always `RACK_MACRO_COUNT` (8) after
/// `RackTrackSnapshot::normalize_macros`, so in practice the macro term is
/// inert; it is hashed anyway so an un-normalized snapshot cannot silently
/// pass as compatible.
fn rack_glyph_signature_of(
    app: &app::App,
    slot_count: usize,
    bindings: impl Iterator<Item = (sequencer::sequencer::InstrumentType, Option<usize>)>,
    macro_count: usize,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    slot_count.hash(&mut hasher);
    for (instrument_type, engine_id) in bindings {
        std::mem::discriminant(&instrument_type).hash(&mut hasher);
        engine_id.hash(&mut hasher);
        rack_slot_binding_descriptor(app, instrument_type, engine_id)
            .map(|descriptor| descriptor.params.len())
            .hash(&mut hasher);
    }
    macro_count.hash(&mut hasher);
    hasher.finish()
}

fn rack_glyph_signature(app: &app::App, rack: &RackGlyphSnapshot) -> u64 {
    rack_glyph_signature_of(
        app,
        rack.slots.len(),
        rack.slots
            .iter()
            .map(|slot| (slot.instrument_type, slot.engine_id)),
        rack.macros.len(),
    )
}

/// The allocation-free arm, used on the per-patch compatibility path.
pub(super) fn rack_snapshot_glyph_signature(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
) -> u64 {
    rack_glyph_signature_of(
        app,
        rack.slots.len(),
        rack.slots
            .iter()
            .map(|slot| (slot.instrument_type, slot.track_sound_state.engine_id)),
        rack.macros.len(),
    )
}

/// `hash_descriptor_glyph_inputs` for the composite: every slot descriptor's
/// full schema inputs plus the synthesized mix/macro surface. Only runs on a
/// signature/epoch miss — see `cached_surface_glyph_hash`.
fn hash_rack_glyph_inputs(app: &app::App, rack: &RackGlyphSnapshot, hasher: &mut impl Hasher) {
    rack.slots.len().hash(hasher);
    for slot in &rack.slots {
        std::mem::discriminant(&slot.instrument_type).hash(hasher);
        slot.engine_id.hash(hasher);
        match rack_slot_glyph_descriptor(app, slot) {
            Some(descriptor) => {
                true.hash(hasher);
                hash_descriptor_glyph_inputs(descriptor, hasher);
            }
            None => false.hash(hasher),
        }
    }
    rack.macros.len().hash(hasher);
}

/// Grafted rack identity skeleton (spec §2.3): one root branch per slot whose
/// children are that slot's own top-level clusters, plus a `macros` root.
/// `identity_branches` then needs no rack knowledge — with ≤ 6 roots it is
/// under `THIN` and expands into `slot{i}/{cluster}` sub-lobes on its own. The
/// `macros` branch also restores the delta glyph's never-empty guarantee for a
/// rack with no slots at all.
fn rack_identity_skeleton(app: &app::App, rack: &RackGlyphSnapshot) -> sequencer::sound_glyph::Skeleton {
    let mut branches = Vec::new();
    for (index, slot) in rack.slots.iter().enumerate() {
        // A registered engine's authored source is the real silhouette; an
        // unregistered slot falls back to the descriptor's stock radial
        // skeleton. Unlike the flat path there is no instrument NAME to fall
        // back to on a rack slot (`TrackSoundState` carries only the engine
        // id), so there is no disk read here at all.
        let source = slot
            .engine_id
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| engine.source.clone());
        let child = match source {
            Some(source) => sequencer::sound_glyph::extract_skeleton(&source).skeleton,
            None => match rack_slot_glyph_descriptor(app, slot) {
                Some(descriptor) => sequencer::sound_glyph::stock_skeleton(descriptor).skeleton,
                None => sequencer::sound_glyph::Skeleton::default(),
            },
        };
        let weight = child
            .branches
            .iter()
            .map(|branch| branch.weight)
            .sum::<usize>()
            .max(1);
        branches.push(sequencer::sound_glyph::Branch {
            cluster: format!("slot{index}"),
            weight,
            children: child.branches,
        });
    }
    branches.push(sequencer::sound_glyph::Branch {
        cluster: "macros".to_string(),
        weight: rack.macros.len().max(1),
        children: Vec::new(),
    });
    sequencer::sound_glyph::Skeleton { branches }
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

/// Rack arm of the identity cache key (spec §2.3). The flat probe can never
/// fire for a rack — there is no custom instrument name, `track_engine_ids` is
/// `None`, and the descriptor is the zero-param empty slot — so racks key on
/// the rack glyph signature instead. The `rack:` prefix keeps the two key
/// spaces disjoint, so a track that becomes (or stops being) a rack always
/// misses.
fn rack_identity_cache_key(rack_signature: u64, registry_epoch: u64) -> String {
    format!("rack:{rack_signature}:{registry_epoch}")
}

/// Which shape a track's glyph is built from. Both feeds resolve this once per
/// track (`resolve_glyph_surface`) and every downstream step — schema,
/// identity, values, compatibility, invalidation fingerprint — branches here
/// and nowhere else, so the rack arm is written once instead of twice.
enum GlyphSource<'a> {
    /// A normal instrument track: one descriptor, values out of
    /// `patch.instrument_slot.defaults`.
    Flat(&'a sequencer::effects::EffectDescriptor),
    /// A rack track: the live rack projection composed slot-per-lobe, paired
    /// with its glyph signature. The signature rides the variant rather than a
    /// parallel `Option<u64>` so the "rack without a signature" state — which
    /// `resolve_glyph_surface` can never produce — is not representable.
    Rack(RackGlyphSnapshot, u64),
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
///
/// Rack tracks take the grafted arm (`rack_identity_skeleton`) and key on the
/// rack glyph signature: a rack owns no track engine and no custom instrument
/// name, so the flat key would be a constant and the silhouette would never
/// refresh when a slot's engine changed.
fn ensure_identity_cached<'a>(
    app: &app::App,
    track: usize,
    source: &GlyphSource<'_>,
    cache: &'a mut HashMap<usize, IdentityCacheEntry>,
) -> &'a IdentityCacheEntry {
    let registry_epoch = app.editor.engine_registry.epoch();
    let custom = match source {
        GlyphSource::Flat(_) => current_custom_instrument_name(app, track),
        GlyphSource::Rack(..) => None,
    };
    let engine_id = app
        .graph
        .track_engine_ids
        .get(track)
        .and_then(|engine_id| *engine_id);
    let cache_key = match source {
        GlyphSource::Rack(_, signature) => rack_identity_cache_key(*signature, registry_epoch),
        GlyphSource::Flat(descriptor) => identity_cache_key(
            custom.as_deref(),
            descriptor.params.len(),
            engine_id,
            registry_epoch,
        ),
    };
    if cache.get(&track).map(|entry| entry.cache_key.as_str()) != Some(cache_key.as_str()) {
        let skeleton = match source {
            GlyphSource::Rack(rack, _) => rack_identity_skeleton(app, rack),
            GlyphSource::Flat(descriptor) => {
                let lisp = engine_id
                    .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
                    .map(|engine| engine.source.clone())
                    .or_else(|| {
                        custom
                            .as_deref()
                            .and_then(|name| {
                                sequencer::lisp_host::load_instrument_source(name).ok()
                            })
                    });
                lisp.map(|lisp| sequencer::sound_glyph::extract_skeleton(&lisp).skeleton)
                    .unwrap_or_else(|| {
                        sequencer::sound_glyph::stock_skeleton(descriptor).skeleton
                    })
            }
        };
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
///
/// Racks add a fifth probe, the rack glyph signature, because none of the four
/// above can move when a rack is edited (spec §2.4): the track owns no engine,
/// and its descriptor is permanently the zero-param empty slot.
fn cached_surface_glyph_hash(
    app: &app::App,
    track: usize,
    source: &GlyphSource<'_>,
    cache: &mut HashMap<usize, DescriptorGlyphHashEntry>,
) -> u64 {
    let engine_id = app
        .graph
        .track_engine_ids
        .get(track)
        .and_then(|engine_id| *engine_id);
    let registry_epoch = app.editor.engine_registry.epoch();
    let (name, param_count, rack_signature) = match source {
        GlyphSource::Flat(descriptor) => (descriptor.name.as_str(), descriptor.params.len(), None),
        GlyphSource::Rack(rack, signature) => ("rack", rack.slots.len(), Some(*signature)),
    };
    if let Some(entry) = cache.get(&track) {
        if entry.engine_id == engine_id
            && entry.registry_epoch == registry_epoch
            && entry.name == name
            && entry.param_count == param_count
            && entry.rack_signature == rack_signature
        {
            return entry.hash;
        }
    }
    let mut hasher = DefaultHasher::new();
    match source {
        GlyphSource::Flat(descriptor) => hash_descriptor_glyph_inputs(descriptor, &mut hasher),
        GlyphSource::Rack(rack, _) => hash_rack_glyph_inputs(app, rack, &mut hasher),
    }
    let hash = hasher.finish();
    cache.insert(
        track,
        DescriptorGlyphHashEntry {
            engine_id,
            registry_epoch,
            name: name.to_string(),
            param_count,
            rack_signature,
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

/// Everything a feed needs to turn a track's pool patches into glyph frames,
/// resolved once per track (rack-glyph spec §3 phase 1). Before this seam the
/// palette feed and the mixer pattern-cell feed each open-coded the
/// descriptor/schema/identity/values/compatibility chain, so any new track
/// shape had to be threaded through both — and the two had already drifted
/// once on the invalidation hash.
struct GlyphSurface<'a> {
    app: &'a app::App,
    source: GlyphSource<'a>,
    track_instrument_type: Option<&'a sequencer::sequencer::InstrumentType>,
    /// Hash over every schema input, memoized per track.
    schema_hash: u64,
    identity: &'a [IdentityBranch],
    identity_fingerprint: u64,
}

impl<'a> GlyphSurface<'a> {
    fn schema(&self) -> Vec<ParamSchema> {
        match &self.source {
            GlyphSource::Flat(descriptor) => {
                glyph_schema_for_descriptor(descriptor, "instrument:", 0, None)
            }
            GlyphSource::Rack(rack, _) => rack_glyph_schema(self.app, rack),
        }
    }

    /// Structural compatibility. Flat tracks compare the patch's stored
    /// default count against the descriptor's param count; rack tracks
    /// compare the patch's rack glyph signature against the live rack's — a
    /// slot added, removed or re-bound is exactly as incompatible as a flat
    /// track whose instrument was swapped, and renders the same ringed bare
    /// grid. Muted slots deliberately stay in the schema (the rack *is* those
    /// layers); the mute is a boolean param, so toggling it lights one piece
    /// without reshaping anything.
    fn compatible(&self, patch: &sequencer::sequencer::Patch) -> bool {
        let structural = match &self.source {
            GlyphSource::Flat(descriptor) => {
                patch.instrument_slot.defaults.len() == descriptor.params.len()
            }
            // Hashed straight off the patch's `RackTrackSnapshot`: this runs
            // per pool patch per sync, and projecting first would clone every
            // slot's defaults vector for nothing.
            GlyphSource::Rack(_, signature) => patch.rack_track.as_ref().is_some_and(|rack| {
                rack_snapshot_glyph_signature(self.app, rack) == *signature
            }),
        };
        structural
            && self
                .track_instrument_type
                .is_none_or(|instrument_type| &patch.instrument_type == instrument_type)
    }

    fn values(&self, patch: &sequencer::sequencer::Patch) -> Vec<f32> {
        match &self.source {
            GlyphSource::Flat(descriptor) => patch_glyph_values(patch, descriptor),
            GlyphSource::Rack(..) => patch
                .rack_track
                .as_ref()
                .map(|rack| rack_glyph_values(self.app, &project_rack_glyph(rack)))
                .unwrap_or_default(),
        }
    }

    /// Cheap per-frame dirty fold over a patch's values. Kept separate from
    /// `values` because the steady state runs it for every patch of every
    /// track every sync: the flat arm folds the stored defaults directly (no
    /// descriptor padding, no allocation), which is bit-for-bit what the
    /// pattern-cell feed hashed before this seam existed.
    fn value_fingerprint(&self, patch: &sequencer::sequencer::Patch) -> u64 {
        match &self.source {
            GlyphSource::Flat(_) => {
                values_fingerprint(patch.instrument_slot.defaults.iter().copied())
            }
            GlyphSource::Rack(..) => match &patch.rack_track {
                Some(rack) => {
                    let mut hash = 0xcbf29ce484222325u64;
                    for (index, slot) in rack.slots.iter().enumerate() {
                        // The slot index goes in the fold so two racks whose
                        // identical float runs are partitioned differently
                        // across slots cannot collide.
                        hash = fold_values(hash, [index as f32]);
                        hash = fold_values(hash, slot.instrument_slot.defaults.iter().copied());
                        hash = fold_values(
                            hash,
                            [slot.gain, slot.pan, if slot.mute { 1.0 } else { 0.0 }],
                        );
                    }
                    fold_values(hash, rack.macros.iter().map(|entry| entry.value))
                }
                None => 0,
            },
        }
    }
}

/// Resolve one track's glyph surface. Rack tracks read their LIVE rack
/// snapshot here and nowhere else; every other track takes the flat path
/// unchanged, including the borrow of the shared sampler fallback when the
/// track has no registered descriptor.
fn resolve_glyph_surface<'a>(
    app: &'a app::App,
    track: usize,
    descriptor_hashes: &mut HashMap<usize, DescriptorGlyphHashEntry>,
    identity_cache: &'a mut HashMap<usize, IdentityCacheEntry>,
) -> GlyphSurface<'a> {
    let source = match live_rack_glyph_snapshot(app, track) {
        Some(rack) => {
            let signature = rack_glyph_signature(app, &rack);
            GlyphSource::Rack(rack, signature)
        }
        None => GlyphSource::Flat(
            app.graph
                .instrument_descriptors
                .get(track)
                .unwrap_or_else(|| fallback_sampler_descriptor()),
        ),
    };
    let schema_hash = cached_surface_glyph_hash(app, track, &source, descriptor_hashes);
    let identity_entry = ensure_identity_cached(app, track, &source, identity_cache);
    GlyphSurface {
        app,
        source,
        track_instrument_type: app.graph.track_instrument_types.get(track),
        schema_hash,
        identity_fingerprint: identity_entry.fingerprint,
        identity: &identity_entry.branches,
    }
}

/// Resolve cohort statistics once and publish every palette tile from the
/// same reference/cohort snapshot.
pub(super) fn sync_glyph_frames(
    app: &app::App,
    track: usize,
    entries: &[PaletteEntry],
    glyphs: &mut GlyphFrames,
) {
    let pending = collect_glyph_frames(app, track, entries, glyphs);
    publish_sound_glyph_frames(pending);
}

/// Everything `sync_glyph_frames` does except the final publish, returning the
/// frames it would publish.
///
/// The split is a TEST seam, and it exists because reading the frames back out
/// of `sound_glyph_data` is not sound in a test: the store is process-wide and
/// `retain_sound_glyph_frames` deletes every key under the prefix that is not
/// in the caller's active set, so any parallel test whose harness reaches a
/// sync can delete the frames under another test's feet. Tests assert on the
/// returned vector instead.
pub(super) fn collect_glyph_frames(
    app: &app::App,
    track: usize,
    entries: &[PaletteEntry],
    glyphs: &mut GlyphFrames,
) -> Vec<(String, SoundGlyphFrame)> {
    // Disjoint field reborrows so the cached identity branches stay borrowed
    // (inside the surface) while the closure below writes the sibling fields.
    let GlyphFrames { identity: identity_cache, published, revision, descriptor_hashes, .. } =
        glyphs;
    let surface = resolve_glyph_surface(app, track, descriptor_hashes, identity_cache);
    let descriptor_hash = surface.schema_hash;
    let identity_fingerprint = surface.identity_fingerprint;
    let identity = surface.identity;
    let mut active = HashSet::new();
    let mut pending = Vec::new();
    app.state.with_project_scenes(|scenes| {
        let Some(pool) = scenes.track_pools.get(track) else { return };
        // ONE pass over the entries: `compatible` and `values` are resolved
        // exactly once per patch per sync. Both are per-patch work — for a
        // rack, `values` allocates the whole composite vector and `compatible`
        // hashes the patch's slot lineup — and the two of them used to run
        // three times per entry between the cohort filter and the pass below.
        struct Resolved {
            patch: sequencer::sequencer::PatchId,
            is_compatible: bool,
            values: Vec<f32>,
        }
        let resolved = entries.iter().filter_map(|entry| {
            let patch = pool.sounds.patches.get(&entry.patch)?;
            Some(Resolved {
                patch: entry.patch,
                is_compatible: surface.compatible(patch),
                values: surface.values(patch),
            })
        }).collect::<Vec<_>>();
        // Anchor mode (spec §7): the reference is the FIRST patch in palette order,
        // not the selection. Diffing against the selection made every tile change
        // shape on every click, which is disorienting exactly while scanning — and
        // it forced a full cohort re-stat on each selection change.
        let anchor_patch = resolved
            .iter()
            .find(|entry| entry.is_compatible)
            .map(|entry| entry.patch);

        let mut cohort_hasher = DefaultHasher::new();
        descriptor_hash.hash(&mut cohort_hasher);
        for entry in resolved.iter().filter(|entry| entry.is_compatible) {
            entry.patch.hash(&mut cohort_hasher);
            values_fingerprint(entry.values.iter().copied()).hash(&mut cohort_hasher);
        }
        identity_fingerprint.hash(&mut cohort_hasher);
        // The anchor is the first compatible entry, already hashed above, so nothing
        // extra is needed here — and notably the selection is NOT part of the key.
        let cohort_fingerprint = cohort_hasher.finish();

        // Pass 1: fingerprint every tile and collect the misses, so the
        // steady state (palette open, nothing changing) never constructs the
        // schema or re-runs the cohort statistics.
        struct Miss {
            key: String,
            fingerprint: u64,
            /// Index into `resolved`, so the values vector is never cloned for
            /// a tile that turns out to be a cache hit.
            index: usize,
            is_anchor: bool,
            is_compatible: bool,
        }
        let mut misses = Vec::new();
        for (index, entry) in resolved.iter().enumerate() {
            let key = glyph_key(track, entry.patch.0);
            active.insert(key.clone());
            let is_anchor = anchor_patch == Some(entry.patch);
            let mut hasher = DefaultHasher::new();
            cohort_fingerprint.hash(&mut hasher);
            entry.patch.hash(&mut hasher);
            is_anchor.hash(&mut hasher);
            entry.is_compatible.hash(&mut hasher);
            values_fingerprint(entry.values.iter().copied()).hash(&mut hasher);
            let fingerprint = hasher.finish();
            if published.get(&key) == Some(&fingerprint) { continue; }
            misses.push(Miss {
                key,
                fingerprint,
                index,
                is_anchor,
                is_compatible: entry.is_compatible,
            });
        }
        if misses.is_empty() {
            return;
        }

        let schema = surface.schema();
        let cohort = resolved
            .iter()
            .filter(|entry| entry.is_compatible)
            .map(|entry| entry.values.clone())
            .collect::<Vec<_>>();
        let reference = cohort.first().cloned().unwrap_or_default();
        let cohort_model =
            DeltaGlyphCohort::new_with_identity(&schema, &cohort, &reference, identity);
        for miss in misses {
            // An incompatible patch is normalized against itself, so it renders as
            // the bare grid plus the incompatible ring; it is also excluded from the
            // cohort statistics so it cannot poison anyone else's scale.
            let delta = if miss.is_compatible {
                cohort_model.build(&resolved[miss.index].values, miss.is_anchor)
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
    retain_sound_glyph_frames("sound-glyph:", &active);
    published.retain(|key, _| active.contains(key));
    pending
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
pub(super) fn sync_pattern_cell_glyph_frames(app: &app::App, glyphs: &mut GlyphFrames) {
    let pending = collect_pattern_cell_glyph_frames(app, glyphs);
    if !pending.is_empty() {
        publish_sound_glyph_frames(pending);
    }
}

/// Everything `sync_pattern_cell_glyph_frames` does except the final publish.
/// Same test seam, and same reason, as `collect_glyph_frames`.
pub(super) fn collect_pattern_cell_glyph_frames(
    app: &app::App,
    glyphs: &mut GlyphFrames,
) -> Vec<(String, SoundGlyphFrame)> {
    let mut active: HashSet<String> = HashSet::new();
    let mut play_keys: HashSet<String> = HashSet::new();
    let mut pending = Vec::new();
    let mut tracks_seen: HashSet<usize> = HashSet::new();
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
        let surface =
            resolve_glyph_surface(app, track, &mut *descriptor_hashes, &mut *identity_cache);
        let descriptor_hash = surface.schema_hash;
        let identity_fingerprint = surface.identity_fingerprint;
        let identity = surface.identity;
        app.state.with_project_scenes(|scenes| {
            let Some(pool) = scenes.track_pools.get(track) else { return };
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

            // Compatibility is resolved ONCE per cohort patch per sync and
            // reused by the cohort build below: for a rack it hashes the
            // patch's whole slot lineup, which is not something to redo in the
            // steady-state fingerprint loop and then again per cohort member.
            let compatibility = patches
                .iter()
                .map(|id| {
                    pool.sounds
                        .patches
                        .get(id)
                        .map(|patch| (surface.value_fingerprint(patch), surface.compatible(patch)))
                })
                .collect::<Vec<_>>();

            let mut hasher = DefaultHasher::new();
            descriptor_hash.hash(&mut hasher);
            identity_fingerprint.hash(&mut hasher);
            for (pattern, patch) in &cell_patches {
                pattern.hash(&mut hasher);
                patch.hash(&mut hasher);
            }
            for (patch, resolved) in patches.iter().zip(&compatibility) {
                patch.hash(&mut hasher);
                if let Some((value_fingerprint, is_compatible)) = resolved {
                    value_fingerprint.hash(&mut hasher);
                    is_compatible.hash(&mut hasher);
                }
            }
            let fingerprint = hasher.finish();
            if cell_published.get(&track) == Some(&fingerprint) {
                return;
            }
            cell_published.insert(track, fingerprint);

            let schema = surface.schema();
            let cohort = patches
                .iter()
                .zip(&compatibility)
                .filter_map(|(id, resolved)| {
                    let patch = pool.sounds.patches.get(id)?;
                    resolved
                        .is_some_and(|(_, is_compatible)| is_compatible)
                        .then(|| (*id, surface.values(patch)))
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
    retain_sound_glyph_frames("pattern-glyph:", &active);
    set_sound_glyph_play_keys("pattern-glyph:", play_keys);
    cell_published.retain(|track, _| tracks_seen.contains(track));
    pending
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
                "track-sound".to_string(),
                Rc::new(RefCell::new(Value::Bool(entry.is_track_sound))),
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

/// Rack composite surface tests (docs/rack-glyph-spec.md §4). These exercise
/// the pure composition helpers; the two feeds' rack behaviour is covered in
/// `state_values::tests` where the rack-panel app fixture lives.
#[cfg(test)]
mod rack_glyph_tests {
    use super::*;
    use sequencer::app::EngineDescriptor;
    use sequencer::lisp_host::{DGenManifest, DGenParam};
    use sequencer::sequencer::{InstrumentType, SequencerState};
    use std::sync::{Arc, Mutex};

    fn param(name: &str) -> DGenParam {
        DGenParam {
            name: name.to_string(),
            cell_id: 0,
            cell_span: 1,
            default: 0.25,
            min: 0.0,
            max: 1.0,
            unit: None,
            hidden: false,
            group: None,
            env: None,
            role: None,
        }
    }

    fn manifest(params: Vec<DGenParam>) -> DGenManifest {
        DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 2,
            process_abi: "dgen-host-abi-v1".to_string(),
            total_memory_slots: 0,
            params,
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    /// A bare app whose only job is to own an engine registry: every rack
    /// helper reaches through `app` for exactly that (slot descriptors and
    /// slot skeleton sources).
    fn test_app() -> app::App {
        let state = Arc::new(SequencerState::new(1, vec![vec![]]));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        app::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    /// Registers a two-param instrument — one real param, one modulation
    /// plumbing param that the schema must mark hidden — and returns its id.
    fn register_engine(app: &mut app::App, name: &str, source: &str) -> usize {
        app.editor.engine_registry.upsert(EngineDescriptor {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest(vec![
                param("filter_cutoff"),
                param("__dgen_mod_active__0"),
            ]),
            lib_index: 0,
            shared_runtime: false,
        })
    }

    fn custom_slot(engine_id: usize, defaults: Vec<f32>) -> RackSlotGlyph {
        RackSlotGlyph {
            instrument_type: InstrumentType::Custom,
            engine_id: Some(engine_id),
            defaults,
            gain: 1.0,
            pan: 0.0,
            mute: false,
        }
    }

    fn sampler_slot() -> RackSlotGlyph {
        RackSlotGlyph {
            instrument_type: InstrumentType::Sampler,
            engine_id: None,
            defaults: Vec::new(),
            gain: 0.5,
            pan: -0.25,
            mute: true,
        }
    }

    /// §2.1: every slot claims one `Other("slot{i}")` lobe, ids are
    /// slot-prefixed, `order` stays globally contiguous across slots, the
    /// three mix params ride each lobe, and the macros land in `Mod` at the
    /// heavier played-surface weight.
    #[test]
    fn rack_schema_gives_each_slot_its_own_lobe() {
        let mut app = test_app();
        let engine = register_engine(&mut app, "layer/", "(def filter_cutoff 1)");
        let rack = RackGlyphSnapshot {
            slots: vec![custom_slot(engine, vec![0.5, 0.0]), sampler_slot()],
            macros: vec![0.0, 0.5],
        };

        let schema = rack_glyph_schema(&app, &rack);

        // Contiguous global ordering: piece selection ranks on it.
        assert!(schema
            .iter()
            .enumerate()
            .all(|(index, entry)| entry.order == index));

        let slot0 = schema
            .iter()
            .filter(|entry| entry.id.starts_with("slot0:"))
            .collect::<Vec<_>>();
        assert!(slot0
            .iter()
            .all(|entry| entry.group == ParamGroup::Other("slot0".to_string())));
        // The engine's own params ride the lobe alongside the standard
        // instrument surface the manifest expands into.
        assert!(slot0.iter().any(|entry| entry.id == "slot0:filter_cutoff"));
        // Mod plumbing is filtered per slot for free, inside the shared
        // descriptor schema builder.
        let plumbing = slot0
            .iter()
            .find(|entry| entry.id == "slot0:__dgen_mod_active__0")
            .expect("plumbing param present but hidden");
        assert!(!plumbing.visible && !plumbing.audio);

        for prefix in ["slot0", "slot1"] {
            for mix in ["gain", "pan", "mute"] {
                let entry = schema
                    .iter()
                    .find(|entry| entry.id == format!("{prefix}:{mix}"))
                    .unwrap_or_else(|| panic!("{prefix}:{mix} missing"));
                assert_eq!(entry.group, ParamGroup::Other(prefix.to_string()));
            }
        }
        assert_eq!(
            schema
                .iter()
                .find(|entry| entry.id == "slot1:mute")
                .map(|entry| entry.kind),
            Some(GlyphParamKind::Boolean)
        );
        // The sampler slot's own descriptor params are in the slot1 lobe too.
        assert!(schema
            .iter()
            .any(|entry| entry.id.starts_with("slot1:")
                && entry.group == ParamGroup::Other("slot1".to_string())
                && !entry.id.ends_with(":gain")));

        let macros = schema
            .iter()
            .filter(|entry| entry.id.starts_with("macro:"))
            .collect::<Vec<_>>();
        assert_eq!(macros.len(), 2);
        assert_eq!(macros[0].id, "macro:0");
        assert!(macros
            .iter()
            .all(|entry| entry.group == ParamGroup::Mod && entry.weight == RACK_MACRO_WEIGHT));
    }

    /// §2.2: values mirror schema order exactly, short stored defaults pad
    /// from the descriptor, mute is 0/1, macros trail.
    #[test]
    fn rack_values_mirror_schema_order_and_pad_defaults() {
        let mut app = test_app();
        let engine = register_engine(&mut app, "layer/", "(def filter_cutoff 1)");
        // Only the first default is stored: the second must pad from the
        // descriptor, exactly like `patch_glyph_values`.
        let rack = RackGlyphSnapshot {
            slots: vec![custom_slot(engine, vec![0.75]), sampler_slot()],
            macros: vec![0.1, 0.9],
        };

        let schema = rack_glyph_schema(&app, &rack);
        let values = rack_glyph_values(&app, &rack);

        assert_eq!(values.len(), schema.len());
        let value_of = |id: &str| {
            let index = schema.iter().position(|entry| entry.id == id).unwrap();
            values[index]
        };
        assert_eq!(value_of("slot0:filter_cutoff"), 0.75);
        assert_eq!(value_of("slot0:__dgen_mod_active__0"), 0.25);
        assert_eq!(value_of("slot0:gain"), 1.0);
        assert_eq!(value_of("slot0:mute"), 0.0);
        assert_eq!(value_of("slot1:gain"), 0.5);
        assert_eq!(value_of("slot1:pan"), -0.25);
        assert_eq!(value_of("slot1:mute"), 1.0);
        assert_eq!(value_of("macro:0"), 0.1);
        assert_eq!(value_of("macro:1"), 0.9);
    }

    /// §2.4: the signature is the compatibility yardstick. It tracks the slot
    /// lineup and the macro count — anything that reshapes the schema — and
    /// deliberately ignores the values, which are what the glyph diffs.
    #[test]
    fn rack_signature_tracks_lineup_not_values() {
        let mut app = test_app();
        let engine = register_engine(&mut app, "layer/", "(def filter_cutoff 1)");
        let other = register_engine(&mut app, "other/", "(def filter_cutoff 2)");
        let base = RackGlyphSnapshot {
            slots: vec![custom_slot(engine, vec![0.5, 0.0]), sampler_slot()],
            macros: vec![0.0, 0.0],
        };
        let signature = rack_glyph_signature(&app, &base);

        let mut moved = base.clone();
        moved.slots[0].defaults[0] = 0.9;
        moved.slots[1].mute = false;
        moved.macros[1] = 1.0;
        assert_eq!(rack_glyph_signature(&app, &moved), signature);

        let mut dropped = base.clone();
        dropped.slots.pop();
        assert_ne!(rack_glyph_signature(&app, &dropped), signature);

        let mut swapped = base.clone();
        swapped.slots[0].engine_id = Some(other);
        assert_ne!(rack_glyph_signature(&app, &swapped), signature);

        let mut retyped = base.clone();
        retyped.slots[0].instrument_type = InstrumentType::Sampler;
        assert_ne!(rack_glyph_signature(&app, &retyped), signature);

        let mut macros = base.clone();
        macros.macros.push(0.0);
        assert_ne!(rack_glyph_signature(&app, &macros), signature);
    }

    /// §2.3: one grafted root per slot plus the macros root; the slot roots
    /// carry the slot instrument's own clusters as children, which
    /// `identity_branches` expands into `slot{i}/…` sub-lobes because a rack
    /// is under `THIN`.
    #[test]
    fn rack_identity_grafts_one_root_per_slot() {
        let mut app = test_app();
        let engine = register_engine(
            &mut app,
            "layer/",
            "(def filter_cutoff 1)\n(def filter_env (+ filter_cutoff 1))\n(def osc_tone 2)",
        );
        let rack = RackGlyphSnapshot {
            slots: vec![custom_slot(engine, vec![0.5, 0.0]), sampler_slot()],
            macros: vec![0.0, 0.0],
        };

        let skeleton = rack_identity_skeleton(&app, &rack);
        assert_eq!(skeleton.branches.len(), 3);
        assert_eq!(skeleton.branches[0].cluster, "slot0");
        assert_eq!(skeleton.branches[1].cluster, "slot1");
        assert_eq!(skeleton.branches[2].cluster, "macros");
        assert!(skeleton.branches.iter().all(|branch| branch.weight >= 1));

        let branches = sequencer::sound_glyph::identity_branches(&skeleton);
        assert!(!branches.is_empty());
        // Three roots is under `THIN`, so `identity_branches` must expand the
        // slot roots into their children — `slot{i}/{cluster}` sub-lobes are
        // the whole point of grafting rather than emitting flat slot discs.
        assert!(
            branches
                .iter()
                .any(|branch| branch.name.starts_with("slot0/")),
            "expected slot0 sub-lobes, got {:?}",
            branches.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
        assert!(branches.iter().any(|branch| branch.name == "macros"));
    }

    /// The never-empty guarantee: a rack with no slots at all still has the
    /// macros branch, so the delta glyph has a silhouette to pad with.
    #[test]
    fn empty_rack_still_yields_the_macros_branch() {
        let app = test_app();
        let rack = RackGlyphSnapshot {
            slots: Vec::new(),
            macros: vec![0.0; 8],
        };
        let skeleton = rack_identity_skeleton(&app, &rack);
        assert_eq!(skeleton.branches.len(), 1);
        assert_eq!(skeleton.branches[0].cluster, "macros");
        assert!(!sequencer::sound_glyph::identity_branches(&skeleton).is_empty());
    }

    /// The rack identity key lives in its own namespace and moves on both of
    /// its ingredients; the flat key (asserted in `glyph_fingerprint_tests`)
    /// is untouched by racks.
    #[test]
    fn rack_identity_cache_key_tracks_signature_and_epoch() {
        assert_eq!(
            rack_identity_cache_key(11, 3),
            rack_identity_cache_key(11, 3)
        );
        assert_ne!(
            rack_identity_cache_key(11, 3),
            rack_identity_cache_key(12, 3)
        );
        assert_ne!(
            rack_identity_cache_key(11, 3),
            rack_identity_cache_key(11, 4)
        );
        assert_ne!(
            rack_identity_cache_key(11, 3),
            identity_cache_key(None, 0, None, 3)
        );
    }
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
