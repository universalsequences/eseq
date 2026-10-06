//! Device panel media and tables (spec §14.2l, stage 7b-4): what the
//! instrument and effect panels read beyond params and the 7b-3 extras.
//!
//! - **Fixed modulators** (`device.modulators`, `modulator`, keyed (device
//!   instance id, index)): an instrument descriptor's fixed modulation
//!   sources, registered with the device's descriptor like its tensors
//!   ([`sync_device_modulators`]).
//! - **Modulator envelope** (`device.modulator-phase`, `-level`): a
//!   modulator instrument's, from the tick's meter cache (copied into
//!   [`KindsShared`]); live, and an observed one keeps the cache polled with
//!   the fx panel hidden (`HostKinds::wants_modulator_meters`).
//! - **Effect tables** (`device.table-name`, `table-options`, `table-mode`,
//!   `table-engine`, `table-data-key`, `ir-name`): a Filter Table's and a
//!   Convolution Reverb's, from the effect node's registries (no `App`;
//!   `EffectTableFields`, shared with the legacy panels), live; the table
//!   asset list is listed and built once per (UI epoch, FX epoch, content
//!   library epoch), the legacy panel's rebuild gate, and pushed to an
//!   observer only when that key moved or it starts observing.
//! - **Sampler media** (`device.sample-buffer`, `sample-duration`,
//!   `start-time`, `end-time`, `slices`, `slice-active`, `onsets`,
//!   `analysis-*`, `downbeat-time`): a track sampler's or a sampler rack
//!   slot's. They need the `App` (the sample's path, the analysis cache), so
//!   they are model fields computed only while observed
//!   ([`HostKinds::sync_sampler_media`]), by group: the sample (path,
//!   buffer), its analysis (the cache's entry and onset table for the
//!   buffer, looked up only when the cache's generation moved, and the
//!   sample rate its seconds are in) and the slices (those, the slice mode
//!   and sensitivity at the displayed step, the slice edits). Each observed
//!   device's keys are compared in place every tick, and only a group that
//!   moved (or a field that starts being observed) is recomputed (sharing
//!   the legacy panels' `sampler_waveform_sample`, `sampler_slices`,
//!   `SamplerAnalysis`) and pushed, a list only when it differs;
//!   `start-time` / `end-time` are compared every tick (a start drag moves
//!   no counter); a rack slot's are read under the rack lock, and the
//!   sample loads after it is released. Every device reads the no-sampler defaults from its
//!   registration ([`push_media_defaults`]); an unobserved one keeps its
//!   last pushed value.

use super::*;
use sequencer::analysis::SamplerSliceEdits;
use std::path::PathBuf;

// ── fixed modulators ───────────────────────────────────────────────────

/// Register `device`'s fixed modulators (missing ones get their fields),
/// drop those past their count, and push `device.modulators`.
pub(super) fn sync_device_modulators(
    pusher: &mut Pusher<'_>,
    device: InstanceId,
    source: &DeviceSource,
) {
    let modulators = &source.desc.desc.instrument_modulators;
    let ids = indexed_children(
        &mut *pusher.rt,
        device,
        MODULATOR,
        modulators.len(),
        |store, id, at| {
            let modulator = &modulators[at];
            store.push(id, f::MODULATOR_DEVICE, Value::Instance(device));
            store.push(id, f::MODULATOR_INDEX, number(at as f64));
            store.push(id, f::MODULATOR_SLOT, number(modulator.slot as f64));
            store.push(
                id,
                f::MODULATOR_LABEL,
                Value::String(modulator.label.clone()),
            );
        },
    );
    pusher.push(device, f::DEVICE_MODULATORS, instance_list(ids));
}

// ── the modulator envelope ─────────────────────────────────────────────

/// `device.modulator-phase` (`phase`) or `-level`: a track instrument's
/// entry of the meter cache (0 for any track that is no modulator); 0 for
/// any other device.
pub(super) fn device_modulator_meter(
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    phase: bool,
) -> f64 {
    if device.device != DeviceSlot::Instrument {
        return 0.0;
    }
    let shared = shared.borrow();
    let cache = match phase {
        true => &shared.modulator_phases,
        false => &shared.modulator_levels,
    };
    cache.get(device.owner).copied().unwrap_or(0.0)
}

// ── effect tables ──────────────────────────────────────────────────────

/// The effect table fields.
const TABLE_KEYS: [FieldKey; 6] = [
    f::DEVICE_TABLE_NAME,
    f::DEVICE_TABLE_OPTIONS,
    f::DEVICE_TABLE_MODE,
    f::DEVICE_TABLE_ENGINE,
    f::DEVICE_TABLE_DATA_KEY,
    f::DEVICE_IR_NAME,
];

pub(super) fn table_keys() -> impl Iterator<Item = FieldKey> {
    TABLE_KEYS.into_iter()
}

pub(super) fn is_table_field(key: FieldKey) -> bool {
    TABLE_KEYS.contains(&key)
}

/// The table options' cache key: (UI epoch, FX epoch, content library
/// epoch), the legacy panel's rebuild gate.
pub(super) type TableOptionsKey = (usize, usize, u64);

/// The table asset stems a Filter Table loads (`table-options`), listed
/// and built into their list at most once per [`TableOptionsKey`] (the
/// listing stats the asset folders); returns the key they are cached
/// under.
fn table_options_key(sources: &KindsHandles, shared: &RefCell<KindsShared>) -> TableOptionsKey {
    let key = (
        sources.ui_epoch.load(Ordering::Relaxed),
        sources.fx_epoch.load(Ordering::Relaxed),
        crate::content_library_epoch(),
    );
    if shared
        .borrow()
        .table_options
        .as_ref()
        .is_some_and(|(seen, _)| *seen == key)
    {
        return key;
    }
    let stems = sequencer::effects::filter_table_asset::list_asset_stems();
    let options = strings(stems.iter());
    let mut shared = shared.borrow_mut();
    shared.table_listings += 1;
    shared.table_options = Some((key, options));
    key
}

/// `device.table-options`: the cached asset list for a Filter Table, else
/// empty.
fn table_options(sources: &KindsHandles, shared: &RefCell<KindsShared>, table: bool) -> Value {
    if !table {
        return Value::List(Vec::new());
    }
    table_options_key(sources, shared);
    let shared = shared.borrow();
    (shared.table_options.as_ref()).map_or(Value::List(Vec::new()), |(_, options)| options.clone())
}

/// `device`'s table fields (as the legacy effect panels: a Filter Table's
/// table, a Convolution Reverb's IR, from its effect node's registries);
/// none for an instrument, a rack slot or another effect.
pub(super) fn device_table_fields(
    sources: &KindsHandles,
    device: &DeviceSource,
) -> EffectTableFields {
    let name = device.desc.desc.name.as_str();
    let is_effect = !matches!(
        device.device,
        DeviceSlot::Instrument | DeviceSlot::RackSlot(_)
    );
    if !is_effect || !EffectTableFields::applies(name) {
        return EffectTableFields::default();
    }
    let node = effect_node(sources, device).map_or(0, |node| node as i32);
    EffectTableFields::of(name, node)
}

/// Text table field `key` (not `table-options`) of `fields`: empty where
/// the device has none.
pub(super) fn table_text(fields: &EffectTableFields, key: FieldKey) -> &str {
    let field = match key {
        f::DEVICE_TABLE_NAME => &fields.name,
        f::DEVICE_TABLE_MODE => &fields.mode,
        f::DEVICE_TABLE_ENGINE => &fields.engine,
        f::DEVICE_TABLE_DATA_KEY => &fields.data_key,
        f::DEVICE_IR_NAME => &fields.ir_name,
        _ => &None,
    };
    field.as_deref().unwrap_or("")
}

/// Table field `key` of `device` (a cold read); `None` for another key.
pub(super) fn table_field(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    key: FieldKey,
) -> Option<Value> {
    if !is_table_field(key) {
        return None;
    }
    let fields = device_table_fields(sources, device);
    Some(match key {
        f::DEVICE_TABLE_OPTIONS => table_options(sources, shared, fields.is_table()),
        key => text(table_text(&fields, key)),
    })
}

impl HostKinds {
    /// An observed device's table fields in `mask` (`bits`: the table
    /// fields' bits): the registries read once; `table-options` pushed only
    /// when its cache key moved or it was just gained (an unobserved
    /// device's entry is dropped by `sync_device_live`).
    pub(super) fn push_table_fields(
        &mut self,
        pusher: &mut Pusher<'_>,
        id: InstanceId,
        source: &DeviceSource,
        mask: u32,
    ) {
        let (sources, shared) = (pusher.sources, pusher.shared);
        let fields = device_table_fields(sources, source);
        for (bit, key) in DEVICE_LIVE.keys.iter().enumerate() {
            if mask & (1 << bit) == 0 {
                continue;
            }
            if *key != f::DEVICE_TABLE_OPTIONS {
                pusher.push_text(id, *key, table_text(&fields, *key));
                continue;
            }
            let now = fields
                .is_table()
                .then(|| table_options_key(sources, shared));
            let seen = self.panel.table_options.insert(id, now);
            pusher.push_computed_if(id, *key, seen != Some(now), || {
                shared.borrow_mut().table_option_pushes += 1;
                table_options(sources, shared, now.is_some())
            });
        }
    }
}

// ── sampler media ──────────────────────────────────────────────────────

/// The sampler media fields, in the observed mask's bit order.
const MEDIA_KEYS: [FieldKey; 12] = [
    f::DEVICE_SAMPLE_BUFFER,
    f::DEVICE_SAMPLE_DURATION,
    f::DEVICE_START_TIME,
    f::DEVICE_END_TIME,
    f::DEVICE_SLICES,
    f::DEVICE_SLICE_ACTIVE,
    f::DEVICE_ONSETS,
    f::DEVICE_ANALYSIS_STATUS,
    f::DEVICE_ANALYSIS_MESSAGE,
    f::DEVICE_ANALYSIS_BPM,
    f::DEVICE_ANALYSIS_CONFIDENCE,
    f::DEVICE_DOWNBEAT_TIME,
];

static MEDIA_NAMES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| MEDIA_KEYS.iter().map(|key| key.1).collect());

/// The media fields by what they derive from (bits of [`MEDIA_KEYS`]): the
/// sample, the selection (compared every tick), the slices and the
/// analysis.
const SAMPLE_BUFFER_BIT: u32 = 1 << 0;
const SAMPLE_DURATION_BIT: u32 = 1 << 1;
const SAMPLE_BITS: u32 = SAMPLE_BUFFER_BIT | SAMPLE_DURATION_BIT;
const SELECTION_BITS: u32 = 1 << 2 | 1 << 3;
const SLICES_BIT: u32 = 1 << 4;
const SLICE_ACTIVE_BIT: u32 = 1 << 5;
const SLICE_BITS: u32 = SLICES_BIT | SLICE_ACTIVE_BIT;
const ANALYSIS_BITS: u32 = 1 << 6 | 1 << 7 | 1 << 8 | 1 << 9 | 1 << 10 | 1 << 11;
const ALL_MEDIA_BITS: u32 = SAMPLE_BITS | SELECTION_BITS | SLICE_BITS | ANALYSIS_BITS;

/// The sampler media pass's state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct MediaState {
    observed: ObservedList,
    /// Per observed device, what its media were last computed from.
    seen: HashMap<InstanceId, MediaSeen>,
    /// Computations per group (a moved key or a newly observed field), for
    /// tests: sample loads, analysis reads, slice computations.
    pub(crate) sample_reads: u64,
    pub(crate) analysis_reads: u64,
    pub(crate) slice_reads: u64,
}

impl MediaState {
    /// Recompute everything at the next tick (a schema change, a hot reload).
    pub(super) fn invalidate(&mut self) {
        self.observed.reset();
        self.seen.clear();
    }
}

/// What one observed device's media were last pushed from.
#[derive(Default)]
struct MediaSeen {
    /// The observed fields when they were pushed (0 before the first push).
    mask: u32,
    /// `None`: no sampler (its fields read the defaults).
    sampler: Option<SamplerSeen>,
}

/// What an observed sampler's media were last computed from, by group.
struct SamplerSeen {
    sample: SampleKey,
    analysis: AnalysisKey,
    slices: SliceKey,
    /// The sample's length in seconds (which scales the selection); `None`
    /// until loaded for the current sample.
    duration: Option<f64>,
}

/// The sample (`sample-buffer`, `sample-duration`).
struct SampleKey {
    path: Option<PathBuf>,
    buffer_id: i32,
}

impl SampleKey {
    fn of(read: &MediaRead<'_>) -> Self {
        Self {
            path: read.path.cloned(),
            buffer_id: read.buffer_id,
        }
    }

    fn matches(&self, read: &MediaRead<'_>) -> bool {
        self.path.as_ref() == read.path && self.buffer_id == read.buffer_id
    }
}

/// The buffer's analysis (`onsets`, `analysis-*`, `downbeat-time`): the
/// cache's entry and onset table, and the sample rate its seconds are in.
/// `generation` (the cache's, which any buffer's change bumps) skips the
/// lookup while it has not moved.
struct AnalysisKey {
    generation: u64,
    sample_rate: u32,
    entry: Option<Arc<sequencer::analysis::AnalysisEntry>>,
    table: Option<Arc<sequencer::analysis::OnsetTableShared>>,
}

impl AnalysisKey {
    fn read(app: &app::App, buffer_id: i32, generation: u64) -> Self {
        let cache = app.sample_analysis.cache();
        Self {
            generation,
            sample_rate: app.graph.sample_rate,
            entry: cache.get(buffer_id),
            table: cache.table(buffer_id),
        }
    }

    /// Whether `self` holds what `other` does (the generation aside).
    fn same(&self, other: &Self) -> bool {
        fn same_arc<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
        }
        self.sample_rate == other.sample_rate
            && same_arc(&self.entry, &other.entry)
            && same_arc(&self.table, &other.table)
    }
}

/// The slices' own inputs (`slices`, `slice-active`; they also follow the
/// sample and its analysis): the slice mode and sensitivity shown at the
/// displayed step, the slot's slice edits as stored (the ones for another
/// sample are filtered out when computed).
struct SliceKey {
    slice_mode: f32,
    sensitivity: f32,
    edits: Option<SamplerSliceEdits>,
}

impl SliceKey {
    fn of(read: &MediaRead<'_>) -> Self {
        Self {
            slice_mode: read.slice_mode.round(),
            sensitivity: read.sensitivity,
            edits: read.edits.cloned(),
        }
    }

    /// Whether `read` is what this key holds; allocates nothing.
    fn matches(&self, read: &MediaRead<'_>) -> bool {
        self.slice_mode.to_bits() == read.slice_mode.round().to_bits()
            && self.sensitivity.to_bits() == read.sensitivity.to_bits()
            && self.edits.as_ref() == read.edits
    }
}

/// A sampler's media inputs, borrowed where they live.
struct MediaRead<'a> {
    path: Option<&'a PathBuf>,
    buffer_id: i32,
    slice_mode: f32,
    sensitivity: f32,
    edits: Option<&'a SamplerSliceEdits>,
    /// The stored start and end shown at the displayed step (0-1).
    selection: (f32, f32),
}

/// `read` over the media inputs of device `device` of track `track`
/// (`None` for a device that is no sampler: a track instrument of another
/// type, a rack slot of another instrument, an effect). A rack slot's are
/// read under the rack lock.
fn with_media_read<R>(
    app: &app::App,
    sources: &KindsHandles,
    (track, device): (usize, DeviceSlot),
    read: impl FnOnce(Option<&MediaRead<'_>>) -> R,
) -> R {
    use sequencer::instruments::sampler::{
        SLOT_PARAM_END, SLOT_PARAM_SLICE_MODE, SLOT_PARAM_SLICE_SENSITIVITY, SLOT_PARAM_START,
    };
    static SAMPLER: std::sync::OnceLock<sequencer::effects::EffectDescriptor> =
        std::sync::OnceLock::new();
    let step = sources.plock_display_step(track);
    match device {
        DeviceSlot::Instrument if track < app.tracks.len() && app.is_sampler_track(track) => {
            let Some(slot) = app.state.pattern.instrument_slots.get(track) else {
                return read(None);
            };
            let desc = (app.graph.instrument_descriptors.get(track)).unwrap_or_else(|| {
                SAMPLER.get_or_init(sequencer::effects::EffectDescriptor::builtin_sampler)
            });
            let edits = slot.sampler_slice_edits.read().unwrap();
            read(Some(&MediaRead {
                path: app.sampler_path_ref_for_track(track),
                buffer_id: app.graph.track_buffer_ids.get(track).copied().unwrap_or(-1),
                slice_mode: sampler_slice_mode(slot, desc, step).unwrap_or(0.0),
                sensitivity: sampler_slice_sensitivity(slot, desc, step).unwrap_or(0.5),
                edits: edits.as_ref(),
                selection: sampler_selection(slot, step),
            }))
        }
        DeviceSlot::RackSlot(slot_idx) => {
            let racks = app.state.pattern.rack_tracks.lock().unwrap();
            let rack = racks.get(track).and_then(Option::as_ref);
            let slot = rack.and_then(|rack| Some((rack, rack.slots.get(slot_idx)?)));
            let sampler = sequencer::sequencer::InstrumentType::Sampler;
            let Some((rack, slot)) = slot.filter(|(_, slot)| slot.instrument_type == sampler)
            else {
                return read(None);
            };
            let desc = app.rack_slot_descriptor(slot);
            let param = |idx| {
                desc.map_or(0.0, |desc| {
                    rack_slot_param_value(rack, slot_idx, slot, desc, idx, step)
                })
            };
            read(Some(&MediaRead {
                path: rack_slot_sample_path(app, slot),
                buffer_id: slot.sample_id.as_ref().map_or(-1, |(buffer, ..)| *buffer),
                slice_mode: param(SLOT_PARAM_SLICE_MODE),
                sensitivity: param(SLOT_PARAM_SLICE_SENSITIVITY),
                edits: slot.instrument_slot.sampler_slice_edits.as_ref(),
                selection: (param(SLOT_PARAM_START), param(SLOT_PARAM_END)),
            }))
        }
        _ => read(None),
    }
}

impl HostKinds {
    /// The observed sampler media (see the module docs): per observed
    /// device, each group's inputs compared in place and only the fields of
    /// a group that moved (or a field that starts being observed)
    /// recomputed and pushed; the selection compared every tick.
    pub(super) fn sync_sampler_media(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let devices = all_device_ids(&self.device_ids, &self.devices);
        let media = &mut self.media;
        (media.observed).refresh(&*pusher.rt, &MEDIA_NAMES, || devices.collect());
        let entries = &media.observed.entries;
        media
            .seen
            .retain(|id, _| entries.iter().any(|(entry, ..)| entry == id));
        let generation = app.sample_analysis.cache().generation();
        let sources = pusher.sources;
        for &(id, mask, _) in entries {
            let device = pusher
                .shared
                .borrow()
                .devices
                .get(&id)
                .map(|source| (source.owner, source.device));
            let Some(device) = device else {
                continue;
            };
            let seen = media.seen.entry(id).or_default();
            let counts = (&mut media.analysis_reads, &mut media.slice_reads);
            // The sample loads after a rack slot's lock is released.
            let after = with_media_read(app, sources, device, |read| {
                let changes = MediaChanges::of(app, seen, mask, read, generation);
                push_media(pusher, app, id, seen, changes, read, counts)
            });
            if let Some(after) = after {
                push_sample_and_selection(pusher, id, seen, after, &mut media.sample_reads);
            }
        }
    }
}

/// What one tick pushes of one observed device's media.
struct MediaChanges {
    mask: u32,
    /// The observed fields to recompute and push (a moved group's or a
    /// newly observed one).
    push: u32,
    /// The analysis key re-read this tick (`None`: the cache did not move).
    analysis: Option<AnalysisKey>,
    sample_moved: bool,
    slices_moved: bool,
}

impl MediaChanges {
    /// Compare `read` (`None`: no sampler) against `seen` under the analysis
    /// cache's `generation`; allocates nothing unless the analysis moved.
    fn of(
        app: &app::App,
        seen: &MediaSeen,
        mask: u32,
        read: Option<&MediaRead<'_>>,
        generation: u64,
    ) -> Self {
        let gained = mask & !seen.mask;
        let (Some(read), prior) = (read, seen.sampler.as_ref()) else {
            // Back to the defaults once, then only what is newly observed.
            let push = if seen.sampler.is_some() { mask } else { gained };
            return Self {
                mask,
                push,
                analysis: None,
                sample_moved: false,
                slices_moved: false,
            };
        };
        let sample_moved = prior.is_none_or(|prior| !prior.sample.matches(read));
        let analysis = match prior {
            Some(prior)
                if !sample_moved
                    && prior.analysis.generation == generation
                    && prior.analysis.sample_rate == app.graph.sample_rate =>
            {
                None
            }
            _ => Some(AnalysisKey::read(app, read.buffer_id, generation)),
        };
        let analysis_moved = match (&analysis, prior) {
            (Some(now), Some(prior)) => !now.same(&prior.analysis),
            (now, _) => now.is_some(),
        };
        let slices_moved = prior.is_none_or(|prior| !prior.slices.matches(read));
        let mut push = gained;
        if sample_moved {
            push |= SAMPLE_BITS;
        }
        if sample_moved || analysis_moved {
            push |= ANALYSIS_BITS;
        }
        if sample_moved || analysis_moved || slices_moved {
            push |= SLICE_BITS;
        }
        Self {
            mask,
            push: push & mask & !SELECTION_BITS,
            analysis,
            sample_moved,
            slices_moved,
        }
    }
}

/// Recompute and push what `changes` says moved of device `id`'s media
/// from `read` (`None`: no sampler, the defaults) but the sample and the
/// selection, and record the keys in `seen`; returns what the sample and
/// selection need, pushed once the inputs are released
/// ([`push_sample_and_selection`]). `counts`: the analysis and slice
/// computation counters.
fn push_media(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    id: InstanceId,
    seen: &mut MediaSeen,
    changes: MediaChanges,
    read: Option<&MediaRead<'_>>,
    (analysis_reads, slice_reads): (&mut u64, &mut u64),
) -> Option<SampleAndSelection> {
    let MediaChanges { mask, push, .. } = changes;
    seen.mask = mask;
    let Some(read) = read else {
        seen.sampler = None;
        push_media_defaults(pusher, id, push | (mask & SELECTION_BITS));
        return None;
    };
    if push & ANALYSIS_BITS != 0 {
        *analysis_reads += 1;
        let analysis = SamplerAnalysis::of(app, read.buffer_id);
        let (bpm, confidence) = analysis.tempo.unwrap_or((0.0, 0.0));
        for (bit, key) in MEDIA_KEYS.iter().enumerate() {
            if push & ANALYSIS_BITS & (1 << bit) == 0 {
                continue;
            }
            let value = match *key {
                f::DEVICE_ONSETS => {
                    pusher.put_numbers(id, *key, &analysis.onsets);
                    continue;
                }
                f::DEVICE_ANALYSIS_STATUS => text(analysis.status),
                f::DEVICE_ANALYSIS_MESSAGE => text(&analysis.message),
                f::DEVICE_ANALYSIS_BPM => number(bpm),
                f::DEVICE_ANALYSIS_CONFIDENCE => number(confidence),
                f::DEVICE_DOWNBEAT_TIME => number(analysis.downbeat.unwrap_or(-1.0)),
                _ => continue,
            };
            pusher.push(id, *key, value);
        }
    }
    if push & SLICE_BITS != 0 {
        *slice_reads += 1;
        let path = read.path.map(PathBuf::as_path);
        let edits = sequencer::analysis::edits_for_sample_path(read.edits, path);
        let (slices, active) = sampler_slices(
            app,
            read.buffer_id,
            read.slice_mode,
            read.sensitivity,
            edits,
        );
        if push & SLICES_BIT != 0 {
            pusher.put_numbers(id, f::DEVICE_SLICES, &slices);
        }
        if push & SLICE_ACTIVE_BIT != 0 {
            pusher.put_numbers(id, f::DEVICE_SLICE_ACTIVE, &active);
        }
    }
    match &mut seen.sampler {
        Some(prior) => {
            if changes.sample_moved {
                prior.sample = SampleKey::of(read);
                prior.duration = None;
            }
            if let Some(analysis) = changes.analysis {
                prior.analysis = analysis;
            }
            if changes.slices_moved {
                prior.slices = SliceKey::of(read);
            }
        }
        None => {
            let generation = app.sample_analysis.cache().generation();
            seen.sampler = Some(SamplerSeen {
                sample: SampleKey::of(read),
                analysis: (changes.analysis)
                    .unwrap_or_else(|| AnalysisKey::read(app, read.buffer_id, generation)),
                slices: SliceKey::of(read),
                duration: None,
            });
        }
    }
    let duration = seen.sampler.as_ref().and_then(|sampler| sampler.duration);
    let load = push & SAMPLE_BITS != 0 || (mask & SELECTION_BITS != 0 && duration.is_none());
    Some(SampleAndSelection {
        load: load.then(|| read.path.cloned()),
        push: push & SAMPLE_BITS,
        selection: (mask & SELECTION_BITS != 0).then_some(read.selection),
    })
}

/// What a sampler's sample and selection fields need once its inputs are
/// released.
struct SampleAndSelection {
    /// The sample's path to load (`Some(None)`: no sample), when the sample
    /// fields moved or the selection needs its duration.
    load: Option<Option<PathBuf>>,
    /// The sample fields to push.
    push: u32,
    /// The stored start and end, when observed.
    selection: Option<(f32, f32)>,
}

/// Load the sample if `after` asks (counting it in `sample_reads`), push
/// its fields and the selection in seconds.
fn push_sample_and_selection(
    pusher: &mut Pusher<'_>,
    id: InstanceId,
    seen: &mut MediaSeen,
    after: SampleAndSelection,
    sample_reads: &mut u64,
) {
    let Some(sampler) = seen.sampler.as_mut() else {
        return;
    };
    if let Some(path) = after.load {
        *sample_reads += 1;
        let sample = sampler_waveform_sample(path.as_deref(), "waveform");
        let seconds = sample
            .as_ref()
            .map_or(1.0, |sample| sample.duration_seconds);
        sampler.duration = Some(seconds);
        if after.push & SAMPLE_BUFFER_BIT != 0 {
            let buffer = sample
                .as_ref()
                .map_or(Value::Nil, |sample| sample.to_value());
            pusher.push(id, f::DEVICE_SAMPLE_BUFFER, buffer);
        }
        if after.push & SAMPLE_DURATION_BIT != 0 {
            pusher.push(id, f::DEVICE_SAMPLE_DURATION, number(seconds));
        }
    }
    if let Some((start, end)) = after.selection {
        let duration = sampler.duration.unwrap_or(1.0);
        let seconds = |stored: f32| number(f64::from(stored) * duration);
        pusher.push(id, f::DEVICE_START_TIME, seconds(start));
        pusher.push(id, f::DEVICE_END_TIME, seconds(end));
    }
}

/// Push the no-sampler media of device `id` in `mask`: what a device that
/// is no sampler (or one never observed) reads. Pushed to every device when
/// it registers, so an unobserved read is the documented default.
pub(super) fn push_media_defaults(pusher: &mut Pusher<'_>, id: InstanceId, mask: u32) {
    for (bit, key) in MEDIA_KEYS.iter().enumerate() {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let value = match *key {
            f::DEVICE_SAMPLE_BUFFER => Value::Nil,
            f::DEVICE_SAMPLE_DURATION => number(1.0),
            f::DEVICE_SLICES | f::DEVICE_SLICE_ACTIVE | f::DEVICE_ONSETS => Value::List(Vec::new()),
            f::DEVICE_ANALYSIS_STATUS => text("none"),
            f::DEVICE_ANALYSIS_MESSAGE => text(""),
            f::DEVICE_DOWNBEAT_TIME => number(-1.0),
            _ => number(0.0),
        };
        pusher.push(id, *key, value);
    }
}

/// [`push_media_defaults`] for every media field.
pub(super) fn push_all_media_defaults(pusher: &mut Pusher<'_>, id: InstanceId) {
    push_media_defaults(pusher, id, ALL_MEDIA_BITS);
}
