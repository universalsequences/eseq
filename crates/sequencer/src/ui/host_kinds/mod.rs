//! The host kinds of `eseq.kinds` (`content/core/modules/kinds.lisp`),
//! published from the sequencer (docs/kind-bindings-spec.md §3.4, §4, §9;
//! stage 4).
//!
//! The host registers keyed instances from the project model (tracks,
//! scenes, banks, devices), re-keys them on reorder, and pushes their
//! `:host` fields with `Runtime::set_instance_field`. Every push compares
//! with the cell first, so only changed values reach readers and slots.
//!
//! Two feeds keep fields current ([`Feed`]):
//!
//! - **Live** fields read shared sequencer state the UI thread can reach
//!   without the `App` (atomics and shared handles: volume, mute, arm,
//!   selection, step state, playheads, meters, transport). They carry the
//!   observed bit (spec D3): the tick computes one only while
//!   `Runtime::host_fields_observed` says a reader or a held `#'` binding
//!   wants it, and a by-value read of an unobserved one asks the reader hook
//!   ([`install_reader`]) for its current value. Playheads and meters cost no
//!   per-step work while nothing observes them.
//! - **Model** fields come from the `App` (names, colors, presets, device
//!   chains, scenes and banks). The hook cannot reach the `App`, so the tick
//!   keeps every registered instance's model fields current, but only when
//!   the model may have moved: a [`ModelRevision`] built from the existing
//!   change counters (UI/FX epochs, pattern epoch, history revision, scene
//!   revision, track registry, theme tint), like the legacy publishers'
//!   epoch gates. The transport's queued scene and launch quantization are
//!   compared every tick.
//!
//! Track instances are keyed by the track registry's stable `TrackId`s
//! (paired with the registry's generation, so a project load replaces
//! them); scenes, banks, buses and groups by their ids (buses and groups are
//! replaced on a project load too, once, when the generation moves). Sends are keyed (track instance id, bus
//! id), one per bus but the main mix. Steps are positional and lazy
//! (spec D2): keyed (track instance id, step index), registered on the first
//! read of `t.steps` (the reader hook, or the tick while `steps` is
//! observed) and dropped when their track goes or its length shrinks below
//! them. Devices are keyed (track instance id, `did`): 0 for the
//! instrument, else the effect's stable instance id, so a reorder keeps
//! them (`DeviceSlot::did`); a track's MIDI effects, drum rack slots and
//! their effects are devices of the track too, a bus's effects devices of
//! the bus (`devices`). Params are keyed (device instance id, index)
//! and lazy like steps (`params`). Mod routes are keyed by their endpoints'
//! stable ids (`RouteKey`), replaced on a project load like tracks. Each
//! track has one `tuning`, keyed (track instance id, 0), whose `degree`s
//! are keyed (tuning instance id, index), registered at the model sync.
//! Arrangement clips are keyed (track instance id, clip id) and pattern
//! cells (track instance id, pattern id); scene spans are positional. Drum
//! rack pads are keyed (group instance id, member `TrackId`), rack clips
//! (group instance id, clip id), grooves (group instance id, clip id; 0 for
//! the rack's own); pool grooves are positional, the instance kept by
//! groove id across reorders and replaced on a project load. A param's
//! modulation lanes are keyed (param instance id, lane), a device's tensors
//! (device instance id, index), p-lock variants (track or instrument device
//! instance id, label index); project macros are positional, kept by macro
//! id (replaced on a project load), a drum rack's macros keyed (rack
//! instrument device instance id, index), macro mappings (macro instance
//! id, position).
//!
//! [`check_schema`] compares [`PUBLISHED`] with the loaded `eseq.kinds`; the
//! tick re-runs it whenever a kind schema changes (a hot reload) and skips
//! mismatched fields until they are fixed.
//!
//! Layout: this module holds the published schema and the tick
//! ([`HostKinds::sync`]); `live` the shared handles, live-field values and
//! the reader hook; `registry` the instance registry helpers and pushes;
//! `tracks`, `steps`, `params`, `scenes`, `mixer` (buses, groups, routes),
//! `settings` (track settings), `arrangement` (song, clips, cells),
//! `racks` (drum rack pads, rack clips, grooves), `devices` (devices
//! beyond the track chain), `panel` (the device panel extras: modulation
//! lanes and display, process mapping, key locks, tensors), `variants`
//! (p-lock variants), `macros` (project and drum rack macros), `lanes`
//! (process lanes) and `presentation` (the browser, the sound palette, the
//! editor and the app's views, from `ui::presented`) the per-kind syncs.

use crate::*;
use eseqlisp::vm::{HostFieldReader, InstanceId, VM};
use std::sync::atomic::AtomicBool;
use std::sync::LazyLock;

mod arrangement;
mod devices;
mod lanes;
mod live;
mod macros;
mod mixer;
mod panel;
mod params;
mod presentation;
mod racks;
mod registry;
mod scenes;
mod settings;
mod steps;
mod tracks;
mod variants;

use arrangement::SongState;
use devices::*;
use lanes::*;
pub(crate) use live::KindsHandles;
use live::*;
use macros::*;
pub(crate) use mixer::KindsMeters;
use mixer::RouteKey;
use panel::*;
use params::*;
use presentation::PresentedState;
use racks::RackState;
use registry::*;
use settings::*;
use steps::*;
use variants::*;

/// The module declaring the host kinds.
pub(crate) const KINDS_MODULE: &str = "eseq.kinds";

pub(crate) const TRACK: &str = "eseq.kinds:track";
pub(crate) const STEP: &str = "eseq.kinds:step";
pub(crate) const DEVICE: &str = "eseq.kinds:device";
pub(crate) const SCENE: &str = "eseq.kinds:scene";
pub(crate) const BANK: &str = "eseq.kinds:bank";
pub(crate) const TRANSPORT: &str = "eseq.kinds:transport";
pub(crate) const SELECTION: &str = "eseq.kinds:selection";
pub(crate) const PROJECT: &str = "eseq.kinds:project";
pub(crate) const SEND: &str = "eseq.kinds:send";
pub(crate) const BUS: &str = "eseq.kinds:bus";
pub(crate) const GROUP: &str = "eseq.kinds:group";
pub(crate) const MASTER: &str = "eseq.kinds:master";
pub(crate) const ENGINE: &str = "eseq.kinds:engine";
pub(crate) const PARAM: &str = "eseq.kinds:param";
pub(crate) const ROUTE: &str = "eseq.kinds:route";
pub(crate) const TUNING: &str = "eseq.kinds:tuning";
pub(crate) const DEGREE: &str = "eseq.kinds:degree";
pub(crate) const SONG: &str = "eseq.kinds:song";
pub(crate) const REGION: &str = "eseq.kinds:region";
pub(crate) const SCENE_SPAN: &str = "eseq.kinds:scene-span";
pub(crate) const CLIP: &str = "eseq.kinds:clip";
pub(crate) const CELL: &str = "eseq.kinds:cell";
pub(crate) const PAD: &str = "eseq.kinds:pad";
pub(crate) const RACK_CLIP: &str = "eseq.kinds:rack-clip";
pub(crate) const GROOVE: &str = "eseq.kinds:groove";
pub(crate) const PAD_GROOVE: &str = "eseq.kinds:pad-groove";
pub(crate) const POOL_GROOVE: &str = "eseq.kinds:pool-groove";
pub(crate) const LIBRARY_GROOVE: &str = "eseq.kinds:library-groove";
pub(crate) const MOD_TARGET: &str = "eseq.kinds:mod-target";
pub(crate) const TENSOR: &str = "eseq.kinds:tensor";
pub(crate) const VARIANT: &str = "eseq.kinds:variant";
pub(crate) const MACRO: &str = "eseq.kinds:macro";
pub(crate) const RACK_MACRO: &str = "eseq.kinds:rack-macro";
pub(crate) const MACRO_MAPPING: &str = "eseq.kinds:macro-mapping";
pub(crate) const PROCESS: &str = "eseq.kinds:process";
pub(crate) const PROCESS_CLASS: &str = "eseq.kinds:process-class";
pub(crate) const PROCESS_LIBRARY: &str = "eseq.kinds:process-library";
pub(crate) const LANE: &str = "eseq.kinds:lane";
pub(crate) const INLET: &str = "eseq.kinds:inlet";
pub(crate) const PORT: &str = "eseq.kinds:port";
pub(crate) const FANOUT: &str = "eseq.kinds:fanout";
pub(crate) const STATE_CELL: &str = "eseq.kinds:state-cell";
pub(crate) const BROWSER: &str = "eseq.kinds:browser";
pub(crate) const PRESET_FILE: &str = "eseq.kinds:preset-file";
pub(crate) const SLOT_PRESETS: &str = "eseq.kinds:slot-presets";
pub(crate) const SOUND: &str = "eseq.kinds:sound";
pub(crate) const SOUND_PALETTE: &str = "eseq.kinds:sound-palette";
pub(crate) const EDITOR: &str = "eseq.kinds:editor";
pub(crate) const EDITOR_MACRO: &str = "eseq.kinds:editor-macro";
pub(crate) const EDITOR_ASSET: &str = "eseq.kinds:editor-asset";
pub(crate) const ASSET_INFO: &str = "eseq.kinds:asset-info";
pub(crate) const LEARN: &str = "eseq.kinds:learn";
pub(crate) const LEARN_PLAN_PARAM: &str = "eseq.kinds:learn-plan-param";
pub(crate) const LEARN_EPOCH_PARAM: &str = "eseq.kinds:learn-epoch-param";
pub(crate) const LEARN_DELTA: &str = "eseq.kinds:learn-delta";
pub(crate) const RETRO: &str = "eseq.kinds:retro";
pub(crate) const RETRO_LANE: &str = "eseq.kinds:retro-lane";
pub(crate) const RETRO_ITEM: &str = "eseq.kinds:retro-item";
pub(crate) const SONG_EXPORT: &str = "eseq.kinds:song-export";
pub(crate) const SETTINGS: &str = "eseq.kinds:settings";
pub(crate) const MIDI_DEVICE: &str = "eseq.kinds:midi-device";
pub(crate) const AGENT: &str = "eseq.kinds:agent";

/// How the host keeps a field current (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feed {
    /// Re-derived from the `App` for every registered instance whenever the
    /// [`ModelRevision`] moves.
    Model,
    /// Computed only while observed; cold reads go through the reader hook.
    Live,
}

use Feed::{Live, Model};

/// A published field: (kind id, field name).
pub(crate) type FieldKey = (&'static str, &'static str);

/// Every published field, by name. [`PUBLISHED`] lists them all.
pub(crate) mod f {
    use super::*;

    pub(crate) const TRACK_INDEX: FieldKey = (TRACK, "index");
    pub(crate) const TRACK_TID: FieldKey = (TRACK, "tid");
    pub(crate) const TRACK_NAME: FieldKey = (TRACK, "name");
    pub(crate) const TRACK_COLOR: FieldKey = (TRACK, "color");
    pub(crate) const TRACK_VOLUME: FieldKey = (TRACK, "volume");
    pub(crate) const TRACK_PEAK: FieldKey = (TRACK, "peak");
    pub(crate) const TRACK_MUTED: FieldKey = (TRACK, "muted");
    pub(crate) const TRACK_AUDIBLE: FieldKey = (TRACK, "audible");
    pub(crate) const TRACK_ARMED: FieldKey = (TRACK, "armed");
    pub(crate) const TRACK_SELECTED: FieldKey = (TRACK, "selected");
    pub(crate) const TRACK_PRESET: FieldKey = (TRACK, "preset");
    pub(crate) const TRACK_NUM_STEPS: FieldKey = (TRACK, "num-steps");
    pub(crate) const TRACK_STEPS: FieldKey = (TRACK, "steps");
    pub(crate) const TRACK_DEVICES: FieldKey = (TRACK, "devices");
    pub(crate) const TRACK_PAN: FieldKey = (TRACK, "pan");
    pub(crate) const TRACK_SOLOED: FieldKey = (TRACK, "soloed");
    pub(crate) const TRACK_COLLAPSED: FieldKey = (TRACK, "collapsed");
    pub(crate) const TRACK_PLAYHEAD: FieldKey = (TRACK, "playhead");
    pub(crate) const TRACK_TIMEBASE: FieldKey = (TRACK, "timebase");
    pub(crate) const TRACK_INSTRUMENT_TYPE: FieldKey = (TRACK, "instrument-type");
    pub(crate) const TRACK_RACK: FieldKey = (TRACK, "rack");
    pub(crate) const TRACK_GROUP: FieldKey = (TRACK, "group");
    pub(crate) const TRACK_SENDS: FieldKey = (TRACK, "sends");
    pub(crate) const TRACK_POLY: FieldKey = (TRACK, "poly");
    pub(crate) const TRACK_MAX_POLYPHONY: FieldKey = (TRACK, "max-polyphony");
    pub(crate) const TRACK_GATE: FieldKey = (TRACK, "gate");
    pub(crate) const TRACK_SUPPORTS_MONO_TRIGGER: FieldKey = (TRACK, "supports-mono-trigger");
    pub(crate) const TRACK_VOICE_PRIORITY: FieldKey = (TRACK, "voice-priority");
    pub(crate) const TRACK_MONO_TRIGGER: FieldKey = (TRACK, "mono-trigger");
    pub(crate) const TRACK_MUTE_GROUP: FieldKey = (TRACK, "mute-group");
    pub(crate) const TRACK_SWING: FieldKey = (TRACK, "swing");
    pub(crate) const TRACK_SWING_RESOLUTION: FieldKey = (TRACK, "swing-resolution");
    pub(crate) const TRACK_FTS: FieldKey = (TRACK, "fts");
    pub(crate) const TRACK_TUNING: FieldKey = (TRACK, "tuning");
    pub(crate) const TRACK_ACCUMULATOR: FieldKey = (TRACK, "accumulator");
    pub(crate) const TRACK_ACCUM_MODE: FieldKey = (TRACK, "accum-mode");
    pub(crate) const TRACK_ACCUM_LIMIT: FieldKey = (TRACK, "accum-limit");
    pub(crate) const TRACK_OUTPUT: FieldKey = (TRACK, "output");
    pub(crate) const TRACK_MOD_OUTPUT: FieldKey = (TRACK, "mod-output");
    pub(crate) const TRACK_MOD_OUT_LEVEL: FieldKey = (TRACK, "mod-out-level");
    pub(crate) const TRACK_MOD_IN: [FieldKey; 4] = [
        (TRACK, "mod-in-1"),
        (TRACK, "mod-in-2"),
        (TRACK, "mod-in-3"),
        (TRACK, "mod-in-4"),
    ];
    pub(crate) const TRACK_BAR_TRANSPOSES: FieldKey = (TRACK, "bar-transposes");
    pub(crate) const TRACK_DELETE_TARGET: FieldKey = (TRACK, "delete-target");
    pub(crate) const TRACK_CLIPS: FieldKey = (TRACK, "clips");
    pub(crate) const TRACK_CELLS: FieldKey = (TRACK, "cells");
    pub(crate) const TRACK_GOVERNED: FieldKey = (TRACK, "governed");
    pub(crate) const TRACK_LATCHED: FieldKey = (TRACK, "latched");
    pub(crate) const TRACK_PAD: FieldKey = (TRACK, "pad");
    pub(crate) const TRACK_MIDI_DEVICES: FieldKey = (TRACK, "midi-devices");
    pub(crate) const TRACK_VARIANTS: FieldKey = (TRACK, "variants");
    pub(crate) const TRACK_PROCESSES: FieldKey = (TRACK, "processes");
    pub(crate) const TRACK_LANES: FieldKey = (TRACK, "lanes");
    pub(crate) const TRACK_INSTRUMENT_ID: FieldKey = (TRACK, "instrument-id");

    pub(crate) const BROWSER_TRACK: FieldKey = (BROWSER, "track");
    pub(crate) const BROWSER_INSTRUMENT_KIND: FieldKey = (BROWSER, "instrument-kind");
    pub(crate) const BROWSER_INSTRUMENT: FieldKey = (BROWSER, "instrument");
    pub(crate) const BROWSER_INSTRUMENT_LABEL: FieldKey = (BROWSER, "instrument-label");
    pub(crate) const BROWSER_PRESET: FieldKey = (BROWSER, "preset");
    pub(crate) const BROWSER_PRESETS: FieldKey = (BROWSER, "presets");
    pub(crate) const BROWSER_USER_PRESETS: FieldKey = (BROWSER, "user-presets");
    pub(crate) const BROWSER_SAMPLE: FieldKey = (BROWSER, "sample");
    pub(crate) const BROWSER_SLOTS: FieldKey = (BROWSER, "rack-slots");
    pub(crate) const BROWSER_ENGINES: FieldKey = (BROWSER, "engines");
    pub(crate) const BROWSER_SOUND_PRESETS: FieldKey = (BROWSER, "sound-presets");
    pub(crate) const BROWSER_KIT_PRESETS: FieldKey = (BROWSER, "kit-presets");
    pub(crate) const BROWSER_LIBRARY_EPOCH: FieldKey = (BROWSER, "library-epoch");
    pub(crate) const BROWSER_PREVIEW_PLAYING: FieldKey = (BROWSER, "preview-playing");
    pub(crate) const BROWSER_PREVIEW_POSITION: FieldKey = (BROWSER, "preview-position");

    pub(crate) const PRESET_FILE_INDEX: FieldKey = (PRESET_FILE, "index");
    pub(crate) const PRESET_FILE_TYPE: FieldKey = (PRESET_FILE, "type");
    pub(crate) const PRESET_FILE_ICON: FieldKey = (PRESET_FILE, "icon");
    pub(crate) const PRESET_FILE_NAME: FieldKey = (PRESET_FILE, "name");
    pub(crate) const PRESET_FILE_PATH: FieldKey = (PRESET_FILE, "path");
    pub(crate) const PRESET_FILE_PADS: FieldKey = (PRESET_FILE, "pads");
    pub(crate) const PRESET_FILE_AUTHOR: FieldKey = (PRESET_FILE, "author");
    pub(crate) const PRESET_FILE_TAGS: FieldKey = (PRESET_FILE, "tags");

    pub(crate) const SLOT_PRESETS_INDEX: FieldKey = (SLOT_PRESETS, "index");
    pub(crate) const SLOT_PRESETS_DEVICE: FieldKey = (SLOT_PRESETS, "device");
    pub(crate) const SLOT_PRESETS_INSTRUMENT: FieldKey = (SLOT_PRESETS, "instrument");
    pub(crate) const SLOT_PRESETS_INSTRUMENT_LABEL: FieldKey = (SLOT_PRESETS, "instrument-label");
    pub(crate) const SLOT_PRESETS_PRESETS: FieldKey = (SLOT_PRESETS, "presets");
    pub(crate) const SLOT_PRESETS_USER_PRESETS: FieldKey = (SLOT_PRESETS, "user-presets");
    pub(crate) const SLOT_PRESETS_PRESET: FieldKey = (SLOT_PRESETS, "preset");

    pub(crate) const SOUND_TRACK: FieldKey = (SOUND, "track");
    pub(crate) const SOUND_PATCH_ID: FieldKey = (SOUND, "patch-id");
    pub(crate) const SOUND_MIX_ID: FieldKey = (SOUND, "mix-id");
    pub(crate) const SOUND_NAME: FieldKey = (SOUND, "name");
    pub(crate) const SOUND_REFERENTS: FieldKey = (SOUND, "referents");
    pub(crate) const SOUND_REFERENTS_SHORT: FieldKey = (SOUND, "referents-short");
    pub(crate) const SOUND_BASE: FieldKey = (SOUND, "base");
    pub(crate) const SOUND_TRACK_SOUND: FieldKey = (SOUND, "track-sound");
    pub(crate) const SOUND_CURRENT: FieldKey = (SOUND, "current");
    pub(crate) const SOUND_PRESET: FieldKey = (SOUND, "preset");
    pub(crate) const SOUND_SAMPLE: FieldKey = (SOUND, "sample");
    pub(crate) const SOUND_DIFF_UP: FieldKey = (SOUND, "diff-up");
    pub(crate) const SOUND_DIFF_DOWN: FieldKey = (SOUND, "diff-down");
    pub(crate) const SOUND_COLORED: FieldKey = (SOUND, "colored");
    pub(crate) const SOUND_COLOR: FieldKey = (SOUND, "color");
    pub(crate) const SOUND_GLYPH_KEY: FieldKey = (SOUND, "glyph-key");

    pub(crate) const PALETTE_OPEN: FieldKey = (SOUND_PALETTE, "open");
    pub(crate) const PALETTE_TRACK: FieldKey = (SOUND_PALETTE, "track");
    pub(crate) const PALETTE_TARGET: FieldKey = (SOUND_PALETTE, "target");
    pub(crate) const PALETTE_TARGET_ID: FieldKey = (SOUND_PALETTE, "target-id");
    pub(crate) const PALETTE_INSTRUMENT: FieldKey = (SOUND_PALETTE, "instrument");
    pub(crate) const PALETTE_SOUNDS: FieldKey = (SOUND_PALETTE, "sounds");

    pub(crate) const EDITOR_MODE: FieldKey = (EDITOR, "mode");
    pub(crate) const EDITOR_SURFACE: FieldKey = (EDITOR, "surface");
    pub(crate) const EDITOR_BUFFER: FieldKey = (EDITOR, "buffer");
    pub(crate) const EDITOR_ERROR: FieldKey = (EDITOR, "error");
    pub(crate) const EDITOR_CANCELING: FieldKey = (EDITOR, "canceling");
    pub(crate) const EDITOR_RUN_MODE: FieldKey = (EDITOR, "run-mode");
    pub(crate) const EDITOR_ACTIVE_MACRO: FieldKey = (EDITOR, "active-macro");
    pub(crate) const EDITOR_ACTIVE_MACRO_ACTION: FieldKey = (EDITOR, "active-macro-action");
    pub(crate) const EDITOR_OPEN_MACRO: FieldKey = (EDITOR, "open-macro");
    pub(crate) const EDITOR_PATCH_MACROS: FieldKey = (EDITOR, "patch-macros");
    pub(crate) const EDITOR_LIBRARY_MACROS: FieldKey = (EDITOR, "library-macros");
    pub(crate) const EDITOR_ASSETS: FieldKey = (EDITOR, "assets");
    pub(crate) const EDITOR_SELECTED_ASSET: FieldKey = (EDITOR, "selected-asset");

    pub(crate) const EDITOR_MACRO_NAME: FieldKey = (EDITOR_MACRO, "name");
    pub(crate) const EDITOR_MACRO_LIBRARY: FieldKey = (EDITOR_MACRO, "library");
    pub(crate) const EDITOR_MACRO_PARAMS: FieldKey = (EDITOR_MACRO, "params");
    pub(crate) const EDITOR_MACRO_CALLS: FieldKey = (EDITOR_MACRO, "calls");
    pub(crate) const EDITOR_MACRO_OUTPUTS: FieldKey = (EDITOR_MACRO, "outputs");
    pub(crate) const EDITOR_MACRO_SUMMARY: FieldKey = (EDITOR_MACRO, "summary");
    pub(crate) const EDITOR_MACRO_USED: FieldKey = (EDITOR_MACRO, "used");

    pub(crate) const EDITOR_ASSET_INDEX: FieldKey = (EDITOR_ASSET, "index");
    pub(crate) const EDITOR_ASSET_REFERENCE: FieldKey = (EDITOR_ASSET, "reference");
    pub(crate) const EDITOR_ASSET_TIER: FieldKey = (EDITOR_ASSET, "tier");
    pub(crate) const EDITOR_ASSET_SOURCE_PATH: FieldKey = (EDITOR_ASSET, "source-path");

    pub(crate) const ASSET_REFERENCE: FieldKey = (ASSET_INFO, "reference");
    pub(crate) const ASSET_TENSOR_KIND: FieldKey = (ASSET_INFO, "tensor-kind");
    pub(crate) const ASSET_LAYOUT: FieldKey = (ASSET_INFO, "layout");
    pub(crate) const ASSET_SHAPE: FieldKey = (ASSET_INFO, "shape");
    pub(crate) const ASSET_SOURCE: FieldKey = (ASSET_INFO, "source");
    pub(crate) const ASSET_WAVE_COUNT: FieldKey = (ASSET_INFO, "wave-count");
    pub(crate) const ASSET_WAVES_PER_SET: FieldKey = (ASSET_INFO, "waves-per-set");
    pub(crate) const ASSET_SET_COUNT: FieldKey = (ASSET_INFO, "set-count");
    pub(crate) const ASSET_SETS: FieldKey = (ASSET_INFO, "sets");
    pub(crate) const ASSET_WAVE_NAMES: FieldKey = (ASSET_INFO, "wave-names");

    pub(crate) const LEARN_TARGET_PATH: FieldKey = (LEARN, "target-path");
    pub(crate) const LEARN_TARGET_NAME: FieldKey = (LEARN, "target-name");
    pub(crate) const LEARN_PHASE: FieldKey = (LEARN, "phase");
    pub(crate) const LEARN_METHOD: FieldKey = (LEARN, "method");
    pub(crate) const LEARN_EPOCHS: FieldKey = (LEARN, "epochs");
    pub(crate) const LEARN_CMA_GENERATIONS: FieldKey = (LEARN, "cma-generations");
    pub(crate) const LEARN_CMA_POPULATION: FieldKey = (LEARN, "cma-population");
    pub(crate) const LEARN_CMA_SIGMA: FieldKey = (LEARN, "cma-sigma");
    pub(crate) const LEARN_CMA_SEED: FieldKey = (LEARN, "cma-seed");
    pub(crate) const LEARN_CMA_FORWARD_BATCH: FieldKey = (LEARN, "cma-forward-batch");
    pub(crate) const LEARN_LOCAL_EPOCHS: FieldKey = (LEARN, "local-epochs");
    pub(crate) const LEARN_CMA_CONTINUE: FieldKey = (LEARN, "cma-continue");
    pub(crate) const LEARN_CMA_REFINE_EPOCHS: FieldKey = (LEARN, "cma-refine-epochs");
    pub(crate) const LEARN_CMA_REFINE_MODE: FieldKey = (LEARN, "cma-refine-mode");
    pub(crate) const LEARN_CMA_FINAL_EPOCHS: FieldKey = (LEARN, "cma-final-epochs");
    pub(crate) const LEARN_PITCH_HZ: FieldKey = (LEARN, "pitch-hz");
    pub(crate) const LEARN_GATE_FRAMES: FieldKey = (LEARN, "gate-frames");
    pub(crate) const LEARN_STAGE: FieldKey = (LEARN, "stage");
    pub(crate) const LEARN_CURRENT_EPOCH: FieldKey = (LEARN, "current-epoch");
    pub(crate) const LEARN_TOTAL_EPOCHS: FieldKey = (LEARN, "total-epochs");
    pub(crate) const LEARN_LOSS: FieldKey = (LEARN, "loss");
    pub(crate) const LEARN_LOSSES: FieldKey = (LEARN, "losses");
    pub(crate) const LEARN_OPTIMIZATION_LOSSES: FieldKey = (LEARN, "optimization-losses");
    pub(crate) const LEARN_PLAN_PARAMS: FieldKey = (LEARN, "plan-params");
    pub(crate) const LEARN_EPOCH_PARAMS: FieldKey = (LEARN, "epoch-params");
    pub(crate) const LEARN_IMPROVEMENT_PCT: FieldKey = (LEARN, "improvement-pct");
    pub(crate) const LEARN_ABS_DISTANCE: FieldKey = (LEARN, "abs-distance");
    pub(crate) const LEARN_BASIN_CHECK: FieldKey = (LEARN, "basin-check");
    pub(crate) const LEARN_RESULT_DELTAS: FieldKey = (LEARN, "result-deltas");
    pub(crate) const LEARN_SEEDED_WAV: FieldKey = (LEARN, "seeded-wav");
    pub(crate) const LEARN_FINAL_WAV: FieldKey = (LEARN, "final-wav");
    pub(crate) const LEARN_APPLIED: FieldKey = (LEARN, "applied");
    pub(crate) const LEARN_ERROR: FieldKey = (LEARN, "error");

    pub(crate) const PLAN_PARAM_INDEX: FieldKey = (LEARN_PLAN_PARAM, "index");
    pub(crate) const PLAN_PARAM_NAME: FieldKey = (LEARN_PLAN_PARAM, "name");
    pub(crate) const PLAN_PARAM_STATUS: FieldKey = (LEARN_PLAN_PARAM, "status");
    pub(crate) const PLAN_PARAM_REASON: FieldKey = (LEARN_PLAN_PARAM, "reason");
    pub(crate) const EPOCH_PARAM_INDEX: FieldKey = (LEARN_EPOCH_PARAM, "index");
    pub(crate) const EPOCH_PARAM_NAME: FieldKey = (LEARN_EPOCH_PARAM, "name");
    pub(crate) const EPOCH_PARAM_FROM: FieldKey = (LEARN_EPOCH_PARAM, "from");
    pub(crate) const EPOCH_PARAM_VALUE: FieldKey = (LEARN_EPOCH_PARAM, "value");
    pub(crate) const EPOCH_PARAM_CHANGE: FieldKey = (LEARN_EPOCH_PARAM, "change");
    pub(crate) const EPOCH_PARAM_STEP: FieldKey = (LEARN_EPOCH_PARAM, "step");
    pub(crate) const DELTA_INDEX: FieldKey = (LEARN_DELTA, "index");
    pub(crate) const DELTA_NAME: FieldKey = (LEARN_DELTA, "name");
    pub(crate) const DELTA_FROM: FieldKey = (LEARN_DELTA, "from");
    pub(crate) const DELTA_TO: FieldKey = (LEARN_DELTA, "to");
    pub(crate) const DELTA_CHANGE: FieldKey = (LEARN_DELTA, "change");

    pub(crate) const RETRO_LANES: FieldKey = (RETRO, "lanes");
    pub(crate) const RETRO_ITEMS: FieldKey = (RETRO, "items");
    pub(crate) const RETRO_DURATION: FieldKey = (RETRO, "duration");
    pub(crate) const RETRO_TRUNCATED: FieldKey = (RETRO, "truncated");
    pub(crate) const RETRO_ERROR: FieldKey = (RETRO, "error");
    pub(crate) const RETRO_PLAYING: FieldKey = (RETRO, "playing");
    pub(crate) const RETRO_POSITION: FieldKey = (RETRO, "position");
    pub(crate) const RETRO_LANE_INDEX: FieldKey = (RETRO_LANE, "index");
    pub(crate) const RETRO_LANE_LABEL: FieldKey = (RETRO_LANE, "label");
    pub(crate) const RETRO_ITEM_INDEX: FieldKey = (RETRO_ITEM, "index");
    pub(crate) const RETRO_ITEM_LANE: FieldKey = (RETRO_ITEM, "lane");
    pub(crate) const RETRO_ITEM_START: FieldKey = (RETRO_ITEM, "start");
    pub(crate) const RETRO_ITEM_END: FieldKey = (RETRO_ITEM, "end");

    pub(crate) const EXPORT_DEFAULT_NAME: FieldKey = (SONG_EXPORT, "default-name");
    pub(crate) const EXPORT_PROJECT: FieldKey = (SONG_EXPORT, "project");
    pub(crate) const EXPORT_FOLDER: FieldKey = (SONG_EXPORT, "folder");
    pub(crate) const EXPORT_END: FieldKey = (SONG_EXPORT, "end");
    pub(crate) const EXPORT_BUSY: FieldKey = (SONG_EXPORT, "busy");
    pub(crate) const EXPORT_DONE: FieldKey = (SONG_EXPORT, "done");
    pub(crate) const EXPORT_MESSAGE: FieldKey = (SONG_EXPORT, "message");
    pub(crate) const EXPORT_PERCENT: FieldKey = (SONG_EXPORT, "percent");
    pub(crate) const EXPORT_OUTPUT_NAME: FieldKey = (SONG_EXPORT, "output-name");
    pub(crate) const EXPORT_REVEAL_LABEL: FieldKey = (SONG_EXPORT, "reveal-label");

    pub(crate) const MIDI_DEVICE_INDEX: FieldKey = (MIDI_DEVICE, "index");
    pub(crate) const MIDI_DEVICE_ID: FieldKey = (MIDI_DEVICE, "device-id");
    pub(crate) const MIDI_DEVICE_NAME: FieldKey = (MIDI_DEVICE, "name");
    pub(crate) const MIDI_DEVICE_ENABLED: FieldKey = (MIDI_DEVICE, "enabled");
    pub(crate) const MIDI_DEVICE_CONNECTED: FieldKey = (MIDI_DEVICE, "connected");
    pub(crate) const MIDI_DEVICE_STATUS: FieldKey = (MIDI_DEVICE, "status");

    pub(crate) const SETTINGS_AUDIO_WORKERS_CHOICE: FieldKey = (SETTINGS, "audio-workers-choice");
    pub(crate) const SETTINGS_AUDIO_WORKERS_NOTE: FieldKey = (SETTINGS, "audio-workers-note");
    pub(crate) const SETTINGS_MIDI_DEVICES: FieldKey = (SETTINGS, "midi-devices");
    pub(crate) const SETTINGS_MIDI_ERROR: FieldKey = (SETTINGS, "midi-error");
    pub(crate) const SETTINGS_MIDI_PERSISTENT: FieldKey = (SETTINGS, "midi-persistent");

    pub(crate) const AGENT_GENERATION: FieldKey = (AGENT, "generation");

    pub(crate) const CLASS_INDEX: FieldKey = (PROCESS_CLASS, "index");
    pub(crate) const CLASS_NAME: FieldKey = (PROCESS_CLASS, "name");
    pub(crate) const CLASS_DOC: FieldKey = (PROCESS_CLASS, "doc");
    pub(crate) const CLASS_SOURCE_PATH: FieldKey = (PROCESS_CLASS, "source-path");
    pub(crate) const CLASS_TARGET: FieldKey = (PROCESS_CLASS, "target");
    pub(crate) const CLASS_LANE_COUNT: FieldKey = (PROCESS_CLASS, "lane-count");
    pub(crate) const CLASS_PORTS: FieldKey = (PROCESS_CLASS, "ports");
    pub(crate) const LIBRARY_CLASSES: FieldKey = (PROCESS_LIBRARY, "classes");

    pub(crate) const PROCESS_TRACK: FieldKey = (PROCESS, "track");
    pub(crate) const PROCESS_PROC_ID: FieldKey = (PROCESS, "proc-id");
    pub(crate) const PROCESS_INDEX: FieldKey = (PROCESS, "index");
    pub(crate) const PROCESS_CLASS_REF: FieldKey = (PROCESS, "class");
    pub(crate) const PROCESS_CLASS_NAME: FieldKey = (PROCESS, "class-name");
    pub(crate) const PROCESS_NAME: FieldKey = (PROCESS, "name");
    pub(crate) const PROCESS_INSTANCE_NAME: FieldKey = (PROCESS, "instance-name");
    pub(crate) const PROCESS_PROJECT: FieldKey = (PROCESS, "project");
    pub(crate) const PROCESS_DEFAULT_LANE: FieldKey = (PROCESS, "default-lane");
    pub(crate) const PROCESS_ROSTER: FieldKey = (PROCESS, "roster");
    pub(crate) const PROCESS_ENABLED: FieldKey = (PROCESS, "enabled");
    pub(crate) const PROCESS_DOC: FieldKey = (PROCESS, "doc");
    pub(crate) const PROCESS_SOURCE_PATH: FieldKey = (PROCESS, "source-path");
    pub(crate) const PROCESS_TARGET: FieldKey = (PROCESS, "target");
    pub(crate) const PROCESS_LANES: FieldKey = (PROCESS, "lanes");
    pub(crate) const PROCESS_INLETS: FieldKey = (PROCESS, "inlets");
    pub(crate) const PROCESS_PORTS: FieldKey = (PROCESS, "ports");
    pub(crate) const PROCESS_IN_PORTS: FieldKey = (PROCESS, "in-ports");
    pub(crate) const PROCESS_CELLS: FieldKey = (PROCESS, "cells");
    pub(crate) const PROCESS_EXPR: FieldKey = (PROCESS, "expr");
    pub(crate) const PROCESS_EXPR_LINE: FieldKey = (PROCESS, "expr-line");
    pub(crate) const PROCESS_COMPILE_ERROR: FieldKey = (PROCESS, "compile-error");
    pub(crate) const PROCESS_ERROR: FieldKey = (PROCESS, "error");

    pub(crate) const LANE_PROCESS: FieldKey = (LANE, "process");
    pub(crate) const LANE_TRACK: FieldKey = (LANE, "track");
    pub(crate) const LANE_INDEX: FieldKey = (LANE, "index");
    pub(crate) const LANE_POSITION: FieldKey = (LANE, "position");
    pub(crate) const LANE_INLET: FieldKey = (LANE, "inlet");
    pub(crate) const LANE_LABEL: FieldKey = (LANE, "label");
    pub(crate) const LANE_SHORT_LABEL: FieldKey = (LANE, "short-label");
    pub(crate) const LANE_TYPE: FieldKey = (LANE, "type");
    pub(crate) const LANE_MIN: FieldKey = (LANE, "min");
    pub(crate) const LANE_MAX: FieldKey = (LANE, "max");
    pub(crate) const LANE_DEFAULT: FieldKey = (LANE, "default");
    pub(crate) const LANE_DECIMALS: FieldKey = (LANE, "decimals");
    pub(crate) const LANE_FORKED: FieldKey = (LANE, "forked");
    pub(crate) const LANE_VALUES: FieldKey = (LANE, "values");

    pub(crate) const INLET_PROCESS: FieldKey = (INLET, "process");
    pub(crate) const INLET_INDEX: FieldKey = (INLET, "index");
    pub(crate) const INLET_NAME: FieldKey = (INLET, "name");
    pub(crate) const INLET_TYPE: FieldKey = (INLET, "type");
    pub(crate) const INLET_OPTIONS: FieldKey = (INLET, "options");
    pub(crate) const INLET_VALUE: FieldKey = (INLET, "value");
    pub(crate) const INLET_DEFAULT: FieldKey = (INLET, "default");
    pub(crate) const INLET_MIN: FieldKey = (INLET, "min");
    pub(crate) const INLET_MAX: FieldKey = (INLET, "max");
    pub(crate) const INLET_DECIMALS: FieldKey = (INLET, "decimals");
    pub(crate) const INLET_DOC: FieldKey = (INLET, "doc");

    pub(crate) const PORT_PROCESS: FieldKey = (PORT, "process");
    pub(crate) const PORT_INDEX: FieldKey = (PORT, "index");
    pub(crate) const PORT_NAME: FieldKey = (PORT, "name");
    pub(crate) const PORT_LABEL: FieldKey = (PORT, "label");
    pub(crate) const PORT_HINT: FieldKey = (PORT, "hint");
    pub(crate) const PORT_TARGET: FieldKey = (PORT, "target");
    pub(crate) const PORT_STATUS: FieldKey = (PORT, "status");
    pub(crate) const PORT_MANUAL: FieldKey = (PORT, "manual");
    pub(crate) const PORT_DISCONNECTED: FieldKey = (PORT, "disconnected");
    pub(crate) const PORT_MAPPABLE: FieldKey = (PORT, "mappable");
    pub(crate) const PORT_CONNECTABLE: FieldKey = (PORT, "connectable");
    pub(crate) const PORT_BINDABLE: FieldKey = (PORT, "bindable");
    pub(crate) const PORT_TARGET_KIND: FieldKey = (PORT, "target-kind");
    pub(crate) const PORT_TARGET_PROCESS: FieldKey = (PORT, "target-process");
    pub(crate) const PORT_TARGET_INLET: FieldKey = (PORT, "target-inlet");
    pub(crate) const PORT_TARGET_STEP_PARAM: FieldKey = (PORT, "target-step-param");
    pub(crate) const PORT_FANOUT: FieldKey = (PORT, "fanout");

    pub(crate) const FANOUT_PORT: FieldKey = (FANOUT, "port");
    pub(crate) const FANOUT_INDEX: FieldKey = (FANOUT, "index");
    pub(crate) const FANOUT_TARGET: FieldKey = (FANOUT, "target");
    pub(crate) const FANOUT_TARGET_PROCESS: FieldKey = (FANOUT, "target-process");
    pub(crate) const FANOUT_TARGET_INLET: FieldKey = (FANOUT, "target-inlet");
    pub(crate) const FANOUT_TARGET_STEP_PARAM: FieldKey = (FANOUT, "target-step-param");
    pub(crate) const FANOUT_LO: FieldKey = (FANOUT, "lo");
    pub(crate) const FANOUT_HI: FieldKey = (FANOUT, "hi");

    pub(crate) const STATE_CELL_PROCESS: FieldKey = (STATE_CELL, "process");
    pub(crate) const STATE_CELL_INDEX: FieldKey = (STATE_CELL, "index");
    pub(crate) const STATE_CELL_NAME: FieldKey = (STATE_CELL, "name");
    pub(crate) const STATE_CELL_VALUES: FieldKey = (STATE_CELL, "values");

    pub(crate) const PAD_GROUP: FieldKey = (PAD, "group");
    pub(crate) const PAD_TRACK: FieldKey = (PAD, "track");
    pub(crate) const PAD_NOTE: FieldKey = (PAD, "note");
    pub(crate) const PAD_LABEL: FieldKey = (PAD, "label");
    pub(crate) const PAD_CHOKE: FieldKey = (PAD, "choke");
    pub(crate) const PAD_ROLE: FieldKey = (PAD, "role");
    pub(crate) const PAD_ROLE_TAG: FieldKey = (PAD, "role-tag");
    pub(crate) const PAD_ROLE_LABEL: FieldKey = (PAD, "role-label");
    pub(crate) const PAD_STANDARD_ROLE: FieldKey = (PAD, "standard-role");
    pub(crate) const PAD_STANDARD_ROLE_LABEL: FieldKey = (PAD, "standard-role-label");
    pub(crate) const PAD_TRIGGERED: FieldKey = (PAD, "triggered");

    pub(crate) const RACK_CLIP_GROUP: FieldKey = (RACK_CLIP, "group");
    pub(crate) const RACK_CLIP_CID: FieldKey = (RACK_CLIP, "cid");
    pub(crate) const RACK_CLIP_INDEX: FieldKey = (RACK_CLIP, "index");
    pub(crate) const RACK_CLIP_NAME: FieldKey = (RACK_CLIP, "name");
    pub(crate) const RACK_CLIP_ACTIVE: FieldKey = (RACK_CLIP, "active");
    pub(crate) const RACK_CLIP_SCENES: FieldKey = (RACK_CLIP, "scenes");
    pub(crate) const RACK_CLIP_GROOVE: FieldKey = (RACK_CLIP, "groove");
    pub(crate) const RACK_CLIP_OWN_GROOVE: FieldKey = (RACK_CLIP, "own-groove");

    pub(crate) const GROOVE_GROUP: FieldKey = (GROOVE, "group");
    pub(crate) const GROOVE_CLIP: FieldKey = (GROOVE, "clip");
    pub(crate) const GROOVE_POOL_GROOVE: FieldKey = (GROOVE, "pool-groove");
    pub(crate) const GROOVE_ENABLED: FieldKey = (GROOVE, "enabled");
    pub(crate) const GROOVE_TIMING: FieldKey = (GROOVE, "timing");
    pub(crate) const GROOVE_VELOCITY: FieldKey = (GROOVE, "velocity");
    pub(crate) const GROOVE_RANDOM: FieldKey = (GROOVE, "random");
    pub(crate) const GROOVE_SCALE: FieldKey = (GROOVE, "scale");
    pub(crate) const GROOVE_GRID: FieldKey = (GROOVE, "grid");
    pub(crate) const GROOVE_SLOTS: FieldKey = (GROOVE, "slots");
    pub(crate) const GROOVE_CELLS: FieldKey = (GROOVE, "cells");
    pub(crate) const GROOVE_MEASURED: FieldKey = (GROOVE, "measured");
    pub(crate) const GROOVE_PADS: FieldKey = (GROOVE, "pads");

    pub(crate) const PAD_GROOVE_GROOVE: FieldKey = (PAD_GROOVE, "groove");
    pub(crate) const PAD_GROOVE_PAD: FieldKey = (PAD_GROOVE, "pad");
    pub(crate) const PAD_GROOVE_AMOUNT: FieldKey = (PAD_GROOVE, "amount");
    pub(crate) const PAD_GROOVE_ENABLED: FieldKey = (PAD_GROOVE, "enabled");
    pub(crate) const PAD_GROOVE_CELLS: FieldKey = (PAD_GROOVE, "cells");
    pub(crate) const PAD_GROOVE_MEASURED: FieldKey = (PAD_GROOVE, "measured");

    pub(crate) const POOL_GROOVE_INDEX: FieldKey = (POOL_GROOVE, "index");
    pub(crate) const POOL_GROOVE_ID: FieldKey = (POOL_GROOVE, "groove-id");
    pub(crate) const POOL_GROOVE_NAME: FieldKey = (POOL_GROOVE, "name");
    pub(crate) const POOL_GROOVE_GRID: FieldKey = (POOL_GROOVE, "grid");
    pub(crate) const POOL_GROOVE_RACKS: FieldKey = (POOL_GROOVE, "racks");

    pub(crate) const LIBRARY_GROOVE_INDEX: FieldKey = (LIBRARY_GROOVE, "index");
    pub(crate) const LIBRARY_GROOVE_CHOICE: FieldKey = (LIBRARY_GROOVE, "choice");
    pub(crate) const LIBRARY_GROOVE_NAME: FieldKey = (LIBRARY_GROOVE, "name");
    pub(crate) const LIBRARY_GROOVE_TIER: FieldKey = (LIBRARY_GROOVE, "tier");

    pub(crate) const CLIP_TRACK: FieldKey = (CLIP, "track");
    pub(crate) const CLIP_CID: FieldKey = (CLIP, "cid");
    pub(crate) const CLIP_START: FieldKey = (CLIP, "start");
    pub(crate) const CLIP_END: FieldKey = (CLIP, "end");
    pub(crate) const CLIP_CELL: FieldKey = (CLIP, "cell");
    pub(crate) const CLIP_TAKE: FieldKey = (CLIP, "take");
    pub(crate) const CLIP_OFFSET: FieldKey = (CLIP, "offset");
    pub(crate) const CLIP_NUM_STEPS: FieldKey = (CLIP, "num-steps");
    pub(crate) const CLIP_LENGTH: FieldKey = (CLIP, "length");
    pub(crate) const CLIP_EVENTS: FieldKey = (CLIP, "events");
    pub(crate) const CLIP_DOT: FieldKey = (CLIP, "dot");
    pub(crate) const CLIP_DOT_COLOR: FieldKey = (CLIP, "dot-color");

    pub(crate) const CELL_TRACK: FieldKey = (CELL, "track");
    pub(crate) const CELL_PID: FieldKey = (CELL, "pid");
    pub(crate) const CELL_ACTIVE: FieldKey = (CELL, "active");
    pub(crate) const CELL_ASSIGNED: FieldKey = (CELL, "assigned");
    pub(crate) const CELL_OVERRIDE: FieldKey = (CELL, "override");
    pub(crate) const CELL_QUEUED: FieldKey = (CELL, "queued");
    pub(crate) const CELL_SELECTED: FieldKey = (CELL, "selected");
    pub(crate) const CELL_BANKS: FieldKey = (CELL, "banks");

    pub(crate) const SCENE_SPAN_INDEX: FieldKey = (SCENE_SPAN, "index");
    pub(crate) const SCENE_SPAN_SCENE: FieldKey = (SCENE_SPAN, "scene");
    pub(crate) const SCENE_SPAN_START: FieldKey = (SCENE_SPAN, "start");
    pub(crate) const SCENE_SPAN_END: FieldKey = (SCENE_SPAN, "end");

    pub(crate) const SONG_EXISTS: FieldKey = (SONG, "exists");
    pub(crate) const SONG_MODE: FieldKey = (SONG, "mode");
    pub(crate) const SONG_RECORDING_KIND: FieldKey = (SONG, "recording-kind");
    pub(crate) const SONG_POSITION: FieldKey = (SONG, "position");
    pub(crate) const SONG_CURSOR: FieldKey = (SONG, "cursor");
    pub(crate) const SONG_END: FieldKey = (SONG, "end");
    pub(crate) const SONG_LOOP: FieldKey = (SONG, "loop");
    pub(crate) const SONG_MANUAL_LATCH: FieldKey = (SONG, "manual-latch");
    pub(crate) const SONG_SCENE_LATCHED: FieldKey = (SONG, "scene-latched");
    pub(crate) const SONG_EDIT_ERROR: FieldKey = (SONG, "edit-error");
    pub(crate) const SONG_CAPTURE_FAILED: FieldKey = (SONG, "capture-failed");
    pub(crate) const SONG_CAPTURE_ERROR: FieldKey = (SONG, "capture-error");
    pub(crate) const SONG_REGION: FieldKey = (SONG, "region");
    pub(crate) const SONG_BOUND_CLIP: FieldKey = (SONG, "bound-clip");
    pub(crate) const SONG_SPANS: FieldKey = (SONG, "spans");

    pub(crate) const REGION_TRACKS: FieldKey = (REGION, "tracks");
    pub(crate) const REGION_START: FieldKey = (REGION, "start");
    pub(crate) const REGION_END: FieldKey = (REGION, "end");
    pub(crate) const REGION_SCENE_LANE: FieldKey = (REGION, "scene-lane");

    pub(crate) const STEP_INDEX: FieldKey = (STEP, "index");
    pub(crate) const STEP_TRACK: FieldKey = (STEP, "track");
    pub(crate) const STEP_ACTIVE: FieldKey = (STEP, "active");
    pub(crate) const STEP_PLAYING: FieldKey = (STEP, "playing");
    pub(crate) const STEP_SELECTED: FieldKey = (STEP, "selected");
    pub(crate) const STEP_HELD: FieldKey = (STEP, "held");
    pub(crate) const STEP_VELOCITY: FieldKey = (STEP, "velocity");
    pub(crate) const STEP_DURATION: FieldKey = (STEP, "duration");
    pub(crate) const STEP_TRANSPOSE: FieldKey = (STEP, "transpose");
    pub(crate) const STEP_DELAY: FieldKey = (STEP, "delay");
    pub(crate) const STEP_RETRIG: FieldKey = (STEP, "retrig");
    pub(crate) const STEP_RETRIG_RATE: FieldKey = (STEP, "retrig-rate");
    pub(crate) const STEP_PAN: FieldKey = (STEP, "pan");
    pub(crate) const STEP_SYNC: FieldKey = (STEP, "sync");
    pub(crate) const STEP_AUX_A: FieldKey = (STEP, "aux-a");
    pub(crate) const STEP_PLOCKED: FieldKey = (STEP, "plocked");
    pub(crate) const STEP_LOCK_KIND: FieldKey = (STEP, "lock-kind");
    pub(crate) const STEP_VARIANT_COLOR: FieldKey = (STEP, "variant-color");

    pub(crate) const SEND_TRACK: FieldKey = (SEND, "track");
    pub(crate) const SEND_BUS: FieldKey = (SEND, "bus");
    pub(crate) const SEND_AMOUNT: FieldKey = (SEND, "amount");
    pub(crate) const SEND_DISPLAY: FieldKey = (SEND, "display");
    pub(crate) const SEND_LOCKED: FieldKey = (SEND, "locked");
    pub(crate) const SEND_HAS_LOCKS: FieldKey = (SEND, "has-locks");

    pub(crate) const BUS_INDEX: FieldKey = (BUS, "index");
    pub(crate) const BUS_BID: FieldKey = (BUS, "bid");
    pub(crate) const BUS_NAME: FieldKey = (BUS, "name");
    pub(crate) const BUS_VOLUME: FieldKey = (BUS, "volume");
    pub(crate) const BUS_MUTED: FieldKey = (BUS, "muted");
    pub(crate) const BUS_SOLOED: FieldKey = (BUS, "soloed");
    pub(crate) const BUS_PEAK: FieldKey = (BUS, "peak");
    pub(crate) const BUS_OUTPUT: FieldKey = (BUS, "output");
    pub(crate) const BUS_OUTPUT_OPTIONS: FieldKey = (BUS, "output-options");
    pub(crate) const BUS_DEVICES: FieldKey = (BUS, "devices");
    pub(crate) const BUS_MOD_IN: [FieldKey; 4] = [
        (BUS, "mod-in-1"),
        (BUS, "mod-in-2"),
        (BUS, "mod-in-3"),
        (BUS, "mod-in-4"),
    ];

    pub(crate) const TUNING_TRACK: FieldKey = (TUNING, "track");
    pub(crate) const TUNING_ON: FieldKey = (TUNING, "on");
    pub(crate) const TUNING_SCALE: FieldKey = (TUNING, "scale");
    pub(crate) const TUNING_CUSTOM: FieldKey = (TUNING, "custom");
    pub(crate) const TUNING_EDITED: FieldKey = (TUNING, "edited");
    pub(crate) const TUNING_ROOT: FieldKey = (TUNING, "root");
    pub(crate) const TUNING_MORPH: FieldKey = (TUNING, "morph");
    pub(crate) const TUNING_MODE: FieldKey = (TUNING, "mode");
    pub(crate) const TUNING_PERIOD: FieldKey = (TUNING, "period");
    pub(crate) const TUNING_DEGREES: FieldKey = (TUNING, "degrees");

    pub(crate) const DEGREE_TUNING: FieldKey = (DEGREE, "tuning");
    pub(crate) const DEGREE_INDEX: FieldKey = (DEGREE, "index");
    pub(crate) const DEGREE_BASE: FieldKey = (DEGREE, "base");
    pub(crate) const DEGREE_OFFSET: FieldKey = (DEGREE, "offset");
    pub(crate) const DEGREE_ENABLED: FieldKey = (DEGREE, "enabled");
    pub(crate) const DEGREE_PITCH: FieldKey = (DEGREE, "pitch");
    pub(crate) const DEGREE_LABEL: FieldKey = (DEGREE, "label");
    pub(crate) const DEGREE_RATIO: FieldKey = (DEGREE, "ratio");

    pub(crate) const ROUTE_INDEX: FieldKey = (ROUTE, "index");
    pub(crate) const ROUTE_SOURCE: FieldKey = (ROUTE, "source");
    pub(crate) const ROUTE_DEST: FieldKey = (ROUTE, "dest");
    pub(crate) const ROUTE_DEST_BUS: FieldKey = (ROUTE, "dest-bus");
    pub(crate) const ROUTE_INPUT: FieldKey = (ROUTE, "input");
    pub(crate) const ROUTE_SELECTED: FieldKey = (ROUTE, "selected");

    pub(crate) const GROUP_INDEX: FieldKey = (GROUP, "index");
    pub(crate) const GROUP_GID: FieldKey = (GROUP, "gid");
    pub(crate) const GROUP_NAME: FieldKey = (GROUP, "name");
    pub(crate) const GROUP_COLOR: FieldKey = (GROUP, "color");
    pub(crate) const GROUP_COLLAPSED: FieldKey = (GROUP, "collapsed");
    pub(crate) const GROUP_RACK: FieldKey = (GROUP, "rack");
    pub(crate) const GROUP_TRACKS: FieldKey = (GROUP, "tracks");
    pub(crate) const GROUP_BUS: FieldKey = (GROUP, "bus");
    pub(crate) const GROUP_RACKS: FieldKey = (GROUP, "racks");
    pub(crate) const GROUP_PARENT: FieldKey = (GROUP, "parent");
    pub(crate) const GROUP_ARMED: FieldKey = (GROUP, "armed");
    pub(crate) const GROUP_PADS: FieldKey = (GROUP, "pads");
    pub(crate) const GROUP_CLIPS: FieldKey = (GROUP, "clips");
    pub(crate) const GROUP_RACK_CLIP: FieldKey = (GROUP, "rack-clip");
    pub(crate) const GROUP_LEGACY: FieldKey = (GROUP, "legacy");
    pub(crate) const GROUP_GROOVE: FieldKey = (GROUP, "groove");

    pub(crate) const DEVICE_TRACK: FieldKey = (DEVICE, "track");
    pub(crate) const DEVICE_BUS: FieldKey = (DEVICE, "bus");
    pub(crate) const DEVICE_ROLE: FieldKey = (DEVICE, "role");
    pub(crate) const DEVICE_SLOT: FieldKey = (DEVICE, "slot");
    pub(crate) const DEVICE_DID: FieldKey = (DEVICE, "did");
    pub(crate) const DEVICE_TYPE: FieldKey = (DEVICE, "type");
    pub(crate) const DEVICE_NAME: FieldKey = (DEVICE, "name");
    pub(crate) const DEVICE_ENABLED: FieldKey = (DEVICE, "enabled");
    pub(crate) const DEVICE_PARAMS: FieldKey = (DEVICE, "params");
    pub(crate) const DEVICE_PLAYHEAD: FieldKey = (DEVICE, "playhead");
    pub(crate) const DEVICE_DEVICES: FieldKey = (DEVICE, "devices");
    pub(crate) const DEVICE_CONTAINER: FieldKey = (DEVICE, "container");
    pub(crate) const DEVICE_VOICES: FieldKey = (DEVICE, "voices");
    pub(crate) const DEVICE_DELETE_TARGET: FieldKey = (DEVICE, "delete-target");
    pub(crate) const DEVICE_BASE_NOTE: FieldKey = (DEVICE, "base-note");
    pub(crate) const DEVICE_MOD_PHASES: FieldKey = (DEVICE, "mod-phases");
    pub(crate) const DEVICE_TENSORS: FieldKey = (DEVICE, "tensors");
    pub(crate) const DEVICE_KEY_LOCKED_NOTES: FieldKey = (DEVICE, "key-locked-notes");
    pub(crate) const DEVICE_VARIANTS: FieldKey = (DEVICE, "variants");
    pub(crate) const DEVICE_MACROS: FieldKey = (DEVICE, "macros");

    pub(crate) const PARAM_DEVICE: FieldKey = (PARAM, "device");
    pub(crate) const PARAM_INDEX: FieldKey = (PARAM, "index");
    pub(crate) const PARAM_NAME: FieldKey = (PARAM, "name");
    pub(crate) const PARAM_MIN: FieldKey = (PARAM, "min");
    pub(crate) const PARAM_MAX: FieldKey = (PARAM, "max");
    pub(crate) const PARAM_DEFAULT: FieldKey = (PARAM, "default");
    pub(crate) const PARAM_OPTIONS: FieldKey = (PARAM, "options");
    pub(crate) const PARAM_TYPE: FieldKey = (PARAM, "type");
    pub(crate) const PARAM_UNIT: FieldKey = (PARAM, "unit");
    pub(crate) const PARAM_VALUE: FieldKey = (PARAM, "value");
    pub(crate) const PARAM_BASE: FieldKey = (PARAM, "base");
    pub(crate) const PARAM_LOCKED: FieldKey = (PARAM, "locked");
    pub(crate) const PARAM_OVERRIDDEN: FieldKey = (PARAM, "overridden");
    pub(crate) const PARAM_HAS_LOCKS: FieldKey = (PARAM, "has-locks");
    pub(crate) const PARAM_TEXT: FieldKey = (PARAM, "text");
    pub(crate) const PARAM_PRINTING: FieldKey = (PARAM, "printing");
    pub(crate) const PARAM_LABEL: FieldKey = (PARAM, "label");
    pub(crate) const PARAM_SECTION: FieldKey = (PARAM, "section");
    pub(crate) const PARAM_MOD_SLOT: FieldKey = (PARAM, "mod-slot");
    pub(crate) const PARAM_VISIBLE: FieldKey = (PARAM, "visible");
    pub(crate) const PARAM_MOD_TARGETS: FieldKey = (PARAM, "mod-targets");
    pub(crate) const PARAM_MOD_OFFSET: FieldKey = (PARAM, "mod-offset");
    pub(crate) const PARAM_MOD_VALUE: FieldKey = (PARAM, "mod-value");
    pub(crate) const PARAM_MOD_SCALE: FieldKey = (PARAM, "mod-scale");
    pub(crate) const PARAM_PROCESS_MAPPED: FieldKey = (PARAM, "process-mapped");
    pub(crate) const PARAM_PROCESS_VALUE: FieldKey = (PARAM, "process-value");
    pub(crate) const PARAM_PROCESS_CLAMPED: FieldKey = (PARAM, "process-clamped");
    pub(crate) const PARAM_KEY_LOCKS: FieldKey = (PARAM, "key-locks");

    pub(crate) const MOD_TARGET_PARAM: FieldKey = (MOD_TARGET, "param");
    pub(crate) const MOD_TARGET_INDEX: FieldKey = (MOD_TARGET, "index");
    pub(crate) const MOD_TARGET_SOURCE: FieldKey = (MOD_TARGET, "source");
    pub(crate) const MOD_TARGET_SLOT: FieldKey = (MOD_TARGET, "slot");
    pub(crate) const MOD_TARGET_DEPTH: FieldKey = (MOD_TARGET, "depth");
    pub(crate) const MOD_TARGET_DEPTH_MIN: FieldKey = (MOD_TARGET, "depth-min");
    pub(crate) const MOD_TARGET_DEPTH_MAX: FieldKey = (MOD_TARGET, "depth-max");
    pub(crate) const MOD_TARGET_UNIT: FieldKey = (MOD_TARGET, "unit");

    pub(crate) const TENSOR_DEVICE: FieldKey = (TENSOR, "device");
    pub(crate) const TENSOR_INDEX: FieldKey = (TENSOR, "index");
    pub(crate) const TENSOR_NAME: FieldKey = (TENSOR, "name");
    pub(crate) const TENSOR_ROWS: FieldKey = (TENSOR, "rows");
    pub(crate) const TENSOR_COLS: FieldKey = (TENSOR, "cols");
    pub(crate) const TENSOR_MIN: FieldKey = (TENSOR, "min");
    pub(crate) const TENSOR_MAX: FieldKey = (TENSOR, "max");
    pub(crate) const TENSOR_VALUES: FieldKey = (TENSOR, "values");
    pub(crate) const TENSOR_BASE: FieldKey = (TENSOR, "base");
    pub(crate) const TENSOR_LOCKED: FieldKey = (TENSOR, "locked");

    pub(crate) const VARIANT_TRACK: FieldKey = (VARIANT, "track");
    pub(crate) const VARIANT_DEVICE: FieldKey = (VARIANT, "device");
    pub(crate) const VARIANT_LABEL: FieldKey = (VARIANT, "label");
    pub(crate) const VARIANT_NAME: FieldKey = (VARIANT, "name");
    pub(crate) const VARIANT_COUNT: FieldKey = (VARIANT, "count");
    pub(crate) const VARIANT_COLOR: FieldKey = (VARIANT, "color");
    pub(crate) const VARIANT_CURRENT: FieldKey = (VARIANT, "current");
    pub(crate) const VARIANT_NOTES: FieldKey = (VARIANT, "notes");

    pub(crate) const MACRO_INDEX: FieldKey = (MACRO, "index");
    pub(crate) const MACRO_MID: FieldKey = (MACRO, "mid");
    pub(crate) const MACRO_KEY: FieldKey = (MACRO, "script-key");
    pub(crate) const MACRO_NAME: FieldKey = (MACRO, "name");
    pub(crate) const MACRO_TYPE: FieldKey = (MACRO, "type");
    pub(crate) const MACRO_VALUE: FieldKey = (MACRO, "value");
    pub(crate) const MACRO_MAPPINGS: FieldKey = (MACRO, "mappings");
    pub(crate) const MACRO_TARGET_SCENE: FieldKey = (MACRO, "target-scene");
    pub(crate) const MACRO_MORPH_PARAMS: FieldKey = (MACRO, "morph-params");
    pub(crate) const MACRO_STEAL_PATTERNS: FieldKey = (MACRO, "steal-patterns");
    pub(crate) const MACRO_QUANTIZE: FieldKey = (MACRO, "quantize");

    pub(crate) const RACK_MACRO_DEVICE: FieldKey = (RACK_MACRO, "device");
    pub(crate) const RACK_MACRO_INDEX: FieldKey = (RACK_MACRO, "index");
    pub(crate) const RACK_MACRO_KEY: FieldKey = (RACK_MACRO, "stable-key");
    pub(crate) const RACK_MACRO_NAME: FieldKey = (RACK_MACRO, "name");
    pub(crate) const RACK_MACRO_VALUE: FieldKey = (RACK_MACRO, "value");
    pub(crate) const RACK_MACRO_BASE: FieldKey = (RACK_MACRO, "base");
    pub(crate) const RACK_MACRO_LOCKED: FieldKey = (RACK_MACRO, "locked");
    pub(crate) const RACK_MACRO_HAS_LOCKS: FieldKey = (RACK_MACRO, "has-locks");
    pub(crate) const RACK_MACRO_MAPPINGS: FieldKey = (RACK_MACRO, "mappings");

    pub(crate) const MAPPING_MACRO: FieldKey = (MACRO_MAPPING, "macro");
    pub(crate) const MAPPING_RACK_MACRO: FieldKey = (MACRO_MAPPING, "rack-macro");
    pub(crate) const MAPPING_INDEX: FieldKey = (MACRO_MAPPING, "index");
    pub(crate) const MAPPING_TARGET: FieldKey = (MACRO_MAPPING, "target");
    pub(crate) const MAPPING_LABEL: FieldKey = (MACRO_MAPPING, "label");
    pub(crate) const MAPPING_MIN: FieldKey = (MACRO_MAPPING, "min");
    pub(crate) const MAPPING_MAX: FieldKey = (MACRO_MAPPING, "max");
    pub(crate) const MAPPING_CURVE: FieldKey = (MACRO_MAPPING, "curve");
    pub(crate) const MAPPING_SUSPENDED: FieldKey = (MACRO_MAPPING, "suspended");

    pub(crate) const SCENE_INDEX: FieldKey = (SCENE, "index");
    pub(crate) const SCENE_NUMBER: FieldKey = (SCENE, "number");
    pub(crate) const SCENE_NAME: FieldKey = (SCENE, "name");
    pub(crate) const SCENE_ACTIVE: FieldKey = (SCENE, "active");
    pub(crate) const SCENE_QUEUED: FieldKey = (SCENE, "queued");
    pub(crate) const SCENE_BANK: FieldKey = (SCENE, "bank");

    pub(crate) const BANK_INDEX: FieldKey = (BANK, "index");
    pub(crate) const BANK_LABEL: FieldKey = (BANK, "label");
    pub(crate) const BANK_SCENES: FieldKey = (BANK, "scenes");
    pub(crate) const BANK_PLAYING: FieldKey = (BANK, "playing");

    pub(crate) const TRANSPORT_PLAYING: FieldKey = (TRANSPORT, "playing");
    pub(crate) const TRANSPORT_RECORDING: FieldKey = (TRANSPORT, "recording");
    pub(crate) const TRANSPORT_SCENE: FieldKey = (TRANSPORT, "scene");
    pub(crate) const TRANSPORT_QUEUED: FieldKey = (TRANSPORT, "queued");
    pub(crate) const TRANSPORT_LAUNCH_QUANTIZE: FieldKey = (TRANSPORT, "launch-quantize");
    pub(crate) const TRANSPORT_BPM: FieldKey = (TRANSPORT, "bpm");
    pub(crate) const TRANSPORT_POSITION: FieldKey = (TRANSPORT, "position");
    pub(crate) const TRANSPORT_METRONOME: FieldKey = (TRANSPORT, "metronome");
    pub(crate) const TRANSPORT_ROLL_MODE: FieldKey = (TRANSPORT, "roll-mode");
    pub(crate) const TRANSPORT_RECORD_QUANTIZE: FieldKey = (TRANSPORT, "record-quantize");
    pub(crate) const TRANSPORT_ROLL_RATE: FieldKey = (TRANSPORT, "roll-rate");
    pub(crate) const TRANSPORT_SEQUENCE_ROLLING: FieldKey = (TRANSPORT, "sequence-rolling");

    pub(crate) const MASTER_PEAK_L: FieldKey = (MASTER, "peak-l");
    pub(crate) const MASTER_PEAK_R: FieldKey = (MASTER, "peak-r");
    pub(crate) const MASTER_RECORDING: FieldKey = (MASTER, "recording");

    pub(crate) const ENGINE_CPU_LOAD: FieldKey = (ENGINE, "cpu-load");
    pub(crate) const ENGINE_LATENCY_MS: FieldKey = (ENGINE, "latency-ms");
    pub(crate) const ENGINE_OVERLOADED: FieldKey = (ENGINE, "overloaded");
    pub(crate) const ENGINE_COMPILING: FieldKey = (ENGINE, "compiling");

    pub(crate) const SELECTION_TRACK: FieldKey = (SELECTION, "track");
    pub(crate) const SELECTION_TRACKS: FieldKey = (SELECTION, "tracks");
    pub(crate) const SELECTION_STEPS: FieldKey = (SELECTION, "steps");
    pub(crate) const SELECTION_CURSOR_STEP: FieldKey = (SELECTION, "cursor-step");
    pub(crate) const SELECTION_EDIT_STEP: FieldKey = (SELECTION, "edit-step");
    pub(crate) const SELECTION_RACK_SLOT: FieldKey = (SELECTION, "rack-slot");
    pub(crate) const SELECTION_AUTO_FOLLOW: FieldKey = (SELECTION, "auto-follow");

    pub(crate) const PROJECT_TRACKS: FieldKey = (PROJECT, "tracks");
    pub(crate) const PROJECT_SCENES: FieldKey = (PROJECT, "scenes");
    pub(crate) const PROJECT_BANKS: FieldKey = (PROJECT, "banks");
    pub(crate) const PROJECT_BUSES: FieldKey = (PROJECT, "buses");
    pub(crate) const PROJECT_GROUPS: FieldKey = (PROJECT, "groups");
    pub(crate) const PROJECT_ROUTES: FieldKey = (PROJECT, "routes");
    pub(crate) const PROJECT_FTS_OPTIONS: FieldKey = (PROJECT, "fts-options");
    pub(crate) const PROJECT_SYNC_OPTIONS: FieldKey = (PROJECT, "sync-options");
    pub(crate) const PROJECT_ACCUMULATOR_OPTIONS: FieldKey = (PROJECT, "accumulator-options");
    pub(crate) const PROJECT_OUTPUT_OPTIONS: FieldKey = (PROJECT, "output-options");
    pub(crate) const PROJECT_STEP_PARAM_OPTIONS: FieldKey = (PROJECT, "step-param-options");
    pub(crate) const PROJECT_GROOVE_POOL: FieldKey = (PROJECT, "groove-pool");
    pub(crate) const PROJECT_GROOVE_LIBRARY: FieldKey = (PROJECT, "groove-library");
    pub(crate) const PROJECT_MACROS: FieldKey = (PROJECT, "macros");
    pub(crate) const PROJECT_NAME: FieldKey = (PROJECT, "name");
    pub(crate) const PROJECT_AUDIO_WORKERS_OPTIONS: FieldKey = (PROJECT, "audio-workers-options");
}

/// Every kind and `:host` field the host publishes, with its type as
/// `eseq.kinds` spells it and its feed, grouped by kind. [`check_schema`]
/// holds the module to this; the live-field loops and the reserved kind
/// names derive from it.
pub(crate) const PUBLISHED: &[(FieldKey, &str, Feed)] = &[
    (f::TRACK_INDEX, ":int", Model),
    (f::TRACK_TID, ":int", Model),
    (f::TRACK_NAME, ":string", Model),
    (f::TRACK_COLOR, ":rgb", Model),
    (f::TRACK_VOLUME, ":number", Live),
    (f::TRACK_PEAK, ":number", Live),
    (f::TRACK_MUTED, ":bool", Live),
    (f::TRACK_AUDIBLE, ":bool", Live),
    (f::TRACK_ARMED, ":bool", Live),
    (f::TRACK_SELECTED, ":bool", Live),
    (f::TRACK_PRESET, ":string", Model),
    (f::TRACK_NUM_STEPS, ":int", Live),
    (f::TRACK_STEPS, "(list-of step)", Live),
    (f::TRACK_DEVICES, "(list-of device)", Model),
    // The device sync (`devices`): MIDI effects, bus effects, drum rack
    // slots and their effects.
    (f::TRACK_MIDI_DEVICES, "(list-of device)", Model),
    // The p-lock variant chip list (`variants`): computed while observed,
    // when the track's p-lock key moved.
    (f::TRACK_VARIANTS, "(list-of variant)", Live),
    (f::TRACK_PAN, ":number", Live),
    (f::TRACK_SOLOED, ":bool", Live),
    (f::TRACK_COLLAPSED, ":bool", Live),
    (f::TRACK_PLAYHEAD, ":int", Live),
    (f::TRACK_TIMEBASE, ":string", Live),
    (f::TRACK_INSTRUMENT_TYPE, ":string", Model),
    (f::TRACK_RACK, ":bool", Model),
    (f::TRACK_GROUP, "group", Model),
    (f::TRACK_SENDS, "(list-of send)", Model),
    // Track settings: read from the `App` at the model sync, pushed when
    // they changed (`settings`).
    (f::TRACK_POLY, ":bool", Model),
    (f::TRACK_MAX_POLYPHONY, ":int", Model),
    (f::TRACK_GATE, ":bool", Model),
    (f::TRACK_SUPPORTS_MONO_TRIGGER, ":bool", Model),
    (f::TRACK_VOICE_PRIORITY, ":string", Model),
    (f::TRACK_MONO_TRIGGER, ":string", Model),
    (f::TRACK_MUTE_GROUP, ":int", Model),
    (f::TRACK_SWING, ":number", Model),
    (f::TRACK_SWING_RESOLUTION, ":string", Model),
    (f::TRACK_FTS, ":string", Model),
    (f::TRACK_TUNING, "tuning", Model),
    (f::TRACK_ACCUMULATOR, ":string", Model),
    (f::TRACK_ACCUM_MODE, ":string", Model),
    (f::TRACK_ACCUM_LIMIT, ":number", Model),
    (f::TRACK_OUTPUT, "bus", Model),
    (f::TRACK_MOD_OUTPUT, ":bool", Model),
    (f::TRACK_MOD_OUT_LEVEL, ":number", Live),
    (f::TRACK_MOD_IN[0], ":number", Live),
    (f::TRACK_MOD_IN[1], ":number", Live),
    (f::TRACK_MOD_IN[2], ":number", Live),
    (f::TRACK_MOD_IN[3], ":number", Live),
    (f::TRACK_BAR_TRANSPOSES, "(list-of :number)", Live),
    (f::TRACK_DELETE_TARGET, ":bool", Live),
    // The arrangement (`arrangement`): clips and cells at the song and cell
    // model syncs, `governed` per tick behind its inputs, `latched` from the
    // shared latch mask.
    (f::TRACK_CLIPS, "(list-of clip)", Model),
    (f::TRACK_CELLS, "(list-of cell)", Model),
    (f::TRACK_GOVERNED, ":int", Model),
    (f::TRACK_LATCHED, ":bool", Live),
    // Drum racks (`racks`): the pads, grooves and pool at the rack sync
    // (the groups, the pool and the instances moved), the rack clips at the
    // rack clip sync (the scenes moved); `armed` and `triggered` live.
    (f::TRACK_PAD, "pad", Model),
    (f::PAD_GROUP, "group", Model),
    (f::PAD_TRACK, "track", Model),
    (f::PAD_NOTE, ":int", Model),
    (f::PAD_LABEL, ":string", Model),
    (f::PAD_CHOKE, ":int", Model),
    (f::PAD_ROLE, ":string", Model),
    (f::PAD_ROLE_TAG, ":string", Model),
    (f::PAD_ROLE_LABEL, ":string", Model),
    (f::PAD_STANDARD_ROLE, ":string", Model),
    (f::PAD_STANDARD_ROLE_LABEL, ":string", Model),
    (f::PAD_TRIGGERED, ":bool", Live),
    (f::RACK_CLIP_GROUP, "group", Model),
    (f::RACK_CLIP_CID, ":int", Model),
    (f::RACK_CLIP_INDEX, ":int", Model),
    (f::RACK_CLIP_NAME, ":string", Model),
    (f::RACK_CLIP_ACTIVE, ":bool", Model),
    (f::RACK_CLIP_SCENES, "(list-of scene)", Model),
    (f::RACK_CLIP_GROOVE, "groove", Model),
    (f::RACK_CLIP_OWN_GROOVE, ":bool", Model),
    (f::GROOVE_GROUP, "group", Model),
    (f::GROOVE_CLIP, "rack-clip", Model),
    (f::GROOVE_POOL_GROOVE, "pool-groove", Model),
    (f::GROOVE_ENABLED, ":bool", Model),
    (f::GROOVE_TIMING, ":number", Model),
    (f::GROOVE_VELOCITY, ":number", Model),
    (f::GROOVE_RANDOM, ":number", Model),
    (f::GROOVE_SCALE, ":number", Model),
    (f::GROOVE_GRID, ":string", Model),
    (f::GROOVE_SLOTS, ":int", Model),
    (f::GROOVE_CELLS, "(list-of :number)", Model),
    (f::GROOVE_MEASURED, "(list-of :bool)", Model),
    (f::GROOVE_PADS, "(list-of pad-groove)", Model),
    (f::PAD_GROOVE_GROOVE, "groove", Model),
    (f::PAD_GROOVE_PAD, "pad", Model),
    (f::PAD_GROOVE_AMOUNT, ":number", Model),
    (f::PAD_GROOVE_ENABLED, ":bool", Model),
    (f::PAD_GROOVE_CELLS, "(list-of :number)", Model),
    (f::PAD_GROOVE_MEASURED, "(list-of :bool)", Model),
    (f::POOL_GROOVE_INDEX, ":int", Model),
    (f::POOL_GROOVE_ID, ":int", Model),
    (f::POOL_GROOVE_NAME, ":string", Model),
    (f::POOL_GROOVE_GRID, ":string", Model),
    (f::POOL_GROOVE_RACKS, "(list-of group)", Model),
    // The library listing when the rack inputs or the UI epoch moved.
    (f::LIBRARY_GROOVE_INDEX, ":int", Model),
    (f::LIBRARY_GROOVE_CHOICE, ":string", Model),
    (f::LIBRARY_GROOVE_NAME, ":string", Model),
    (f::LIBRARY_GROOVE_TIER, ":string", Model),
    (f::CLIP_TRACK, "track", Model),
    (f::CLIP_CID, ":int", Model),
    (f::CLIP_START, ":number", Model),
    (f::CLIP_END, ":number", Model),
    (f::CLIP_CELL, "cell", Model),
    (f::CLIP_TAKE, ":int", Model),
    (f::CLIP_OFFSET, ":number", Model),
    // The source's content: when the committed song, the pattern epoch or
    // the pool content moved.
    (f::CLIP_NUM_STEPS, ":int", Model),
    (f::CLIP_LENGTH, ":number", Model),
    (f::CLIP_EVENTS, "(list-of (list-of :number))", Model),
    (f::CLIP_DOT, ":bool", Model),
    (f::CLIP_DOT_COLOR, ":rgb", Model),
    (f::CELL_TRACK, "track", Model),
    (f::CELL_PID, ":int", Model),
    (f::CELL_ACTIVE, ":bool", Model),
    (f::CELL_ASSIGNED, ":bool", Model),
    (f::CELL_OVERRIDE, ":bool", Model),
    (f::CELL_QUEUED, ":bool", Live),
    (f::CELL_SELECTED, ":bool", Live),
    (f::CELL_BANKS, "(list-of bank)", Model),
    (f::SCENE_SPAN_INDEX, ":int", Model),
    (f::SCENE_SPAN_SCENE, "scene", Model),
    (f::SCENE_SPAN_START, ":number", Model),
    (f::SCENE_SPAN_END, ":number", Model),
    (f::SONG_EXISTS, ":bool", Model),
    // `App` state no counter tracks: compared every tick (`SongPushed`).
    (f::SONG_MODE, ":string", Model),
    (f::SONG_RECORDING_KIND, ":string", Model),
    (f::SONG_POSITION, ":number", Live),
    (f::SONG_CURSOR, ":number", Model),
    (f::SONG_END, ":number", Model),
    (f::SONG_LOOP, ":bool", Model),
    (f::SONG_MANUAL_LATCH, ":bool", Live),
    (f::SONG_SCENE_LATCHED, ":bool", Live),
    (f::SONG_EDIT_ERROR, ":string", Model),
    (f::SONG_CAPTURE_FAILED, ":bool", Model),
    (f::SONG_CAPTURE_ERROR, ":string", Model),
    (f::SONG_REGION, "region", Model),
    (f::SONG_BOUND_CLIP, "clip", Model),
    (f::SONG_SPANS, "(list-of scene-span)", Model),
    (f::REGION_TRACKS, "(list-of track)", Model),
    (f::REGION_START, ":number", Model),
    (f::REGION_END, ":number", Model),
    (f::REGION_SCENE_LANE, ":bool", Model),
    (f::STEP_INDEX, ":int", Model),
    (f::STEP_TRACK, "track", Model),
    (f::STEP_ACTIVE, ":bool", Live),
    (f::STEP_PLAYING, ":bool", Live),
    (f::STEP_SELECTED, ":bool", Live),
    (f::STEP_HELD, ":bool", Live),
    (f::STEP_VELOCITY, ":number", Live),
    (f::STEP_DURATION, ":number", Live),
    (f::STEP_TRANSPOSE, ":number", Live),
    (f::STEP_DELAY, ":number", Live),
    (f::STEP_RETRIG, ":number", Live),
    (f::STEP_RETRIG_RATE, ":number", Live),
    (f::STEP_PAN, ":number", Live),
    (f::STEP_SYNC, ":number", Live),
    (f::STEP_AUX_A, ":number", Live),
    (f::STEP_PLOCKED, ":bool", Live),
    (f::STEP_LOCK_KIND, ":int", Live),
    (f::STEP_VARIANT_COLOR, ":rgb", Live),
    (f::SEND_TRACK, "track", Model),
    (f::SEND_BUS, "bus", Model),
    (f::SEND_AMOUNT, ":number", Live),
    (f::SEND_DISPLAY, ":number", Live),
    (f::SEND_LOCKED, ":bool", Live),
    (f::SEND_HAS_LOCKS, ":bool", Live),
    (f::DEVICE_TRACK, "track", Model),
    (f::DEVICE_BUS, "bus", Model),
    (f::DEVICE_SLOT, ":int", Model),
    (f::DEVICE_DID, ":int", Model),
    (f::DEVICE_ROLE, ":string", Model),
    (f::DEVICE_TYPE, ":string", Model),
    (f::DEVICE_NAME, ":string", Model),
    (f::DEVICE_ENABLED, ":bool", Model),
    (f::DEVICE_DEVICES, "(list-of device)", Model),
    (f::DEVICE_CONTAINER, "device", Model),
    (f::DEVICE_VOICES, ":int", Model),
    (f::DEVICE_DELETE_TARGET, ":bool", Live),
    // Registered and pushed on the first read (the reader hook) or once
    // observed (the tick), then at the model sync.
    (f::DEVICE_PARAMS, "(list-of param)", Model),
    (f::DEVICE_PLAYHEAD, ":number", Live),
    // Panel extras (`panel`): the tensors registered with the device; the
    // rest computed while observed.
    (f::DEVICE_BASE_NOTE, ":number", Live),
    (f::DEVICE_MOD_PHASES, "(list-of :number)", Live),
    (f::DEVICE_TENSORS, "(list-of tensor)", Model),
    (f::DEVICE_KEY_LOCKED_NOTES, "(list-of :int)", Live),
    (f::DEVICE_VARIANTS, "(list-of variant)", Live),
    // The rack macro sync (`macros`), when the rack's macros moved.
    (f::DEVICE_MACROS, "(list-of rack-macro)", Model),
    (f::PARAM_DEVICE, "device", Model),
    (f::PARAM_INDEX, ":int", Model),
    (f::PARAM_NAME, ":string", Model),
    (f::PARAM_MIN, ":number", Model),
    (f::PARAM_MAX, ":number", Model),
    (f::PARAM_DEFAULT, ":number", Model),
    (f::PARAM_OPTIONS, "(list-of :string)", Model),
    (f::PARAM_TYPE, ":string", Model),
    (f::PARAM_UNIT, ":string", Model),
    (f::PARAM_VALUE, ":number", Live),
    (f::PARAM_BASE, ":number", Live),
    (f::PARAM_LOCKED, ":bool", Live),
    (f::PARAM_OVERRIDDEN, ":bool", Live),
    (f::PARAM_HAS_LOCKS, ":bool", Live),
    (f::PARAM_TEXT, ":string", Live),
    (f::PARAM_PRINTING, ":bool", Live),
    // Pushed at registration (the descriptor's); the lanes with the params.
    (f::PARAM_LABEL, ":string", Model),
    (f::PARAM_SECTION, ":string", Model),
    (f::PARAM_MOD_SLOT, ":int", Model),
    (f::PARAM_MOD_TARGETS, "(list-of mod-target)", Model),
    (f::PARAM_VISIBLE, ":bool", Live),
    (f::PARAM_MOD_OFFSET, ":number", Live),
    (f::PARAM_MOD_VALUE, ":number", Live),
    (f::PARAM_MOD_SCALE, ":number", Live),
    (f::PARAM_PROCESS_MAPPED, ":bool", Live),
    (f::PARAM_PROCESS_VALUE, ":number", Live),
    (f::PARAM_PROCESS_CLAMPED, ":bool", Live),
    (f::PARAM_KEY_LOCKS, "(list-of (list-of :number))", Live),
    (f::MOD_TARGET_PARAM, "param", Model),
    (f::MOD_TARGET_INDEX, ":int", Model),
    (f::MOD_TARGET_SOURCE, "param", Model),
    (f::MOD_TARGET_SLOT, ":int", Model),
    (f::MOD_TARGET_DEPTH, "param", Model),
    (f::MOD_TARGET_DEPTH_MIN, ":number", Model),
    (f::MOD_TARGET_DEPTH_MAX, ":number", Model),
    (f::MOD_TARGET_UNIT, ":string", Model),
    (f::TENSOR_DEVICE, "device", Model),
    (f::TENSOR_INDEX, ":int", Model),
    (f::TENSOR_NAME, ":string", Model),
    (f::TENSOR_ROWS, ":int", Model),
    (f::TENSOR_COLS, ":int", Model),
    (f::TENSOR_MIN, ":number", Model),
    (f::TENSOR_MAX, ":number", Model),
    (f::TENSOR_VALUES, "(list-of :number)", Live),
    (f::TENSOR_BASE, "(list-of :number)", Live),
    (f::TENSOR_LOCKED, ":bool", Live),
    // Read from the owner's variant registry, cached under its p-lock key.
    (f::VARIANT_TRACK, "track", Model),
    (f::VARIANT_DEVICE, "device", Model),
    (f::VARIANT_LABEL, ":string", Live),
    (f::VARIANT_NAME, ":string", Live),
    (f::VARIANT_COUNT, ":int", Live),
    (f::VARIANT_COLOR, ":rgb", Live),
    (f::VARIANT_CURRENT, ":bool", Live),
    (f::VARIANT_NOTES, "(list-of :int)", Live),
    // Project macros (`macros`): the structure when it moved, the values
    // compared every tick.
    (f::MACRO_INDEX, ":int", Model),
    (f::MACRO_MID, ":int", Model),
    (f::MACRO_KEY, ":string", Model),
    (f::MACRO_NAME, ":string", Model),
    (f::MACRO_TYPE, ":string", Model),
    (f::MACRO_VALUE, ":number", Model),
    (f::MACRO_MAPPINGS, "(list-of macro-mapping)", Model),
    (f::MACRO_TARGET_SCENE, "scene", Model),
    (f::MACRO_MORPH_PARAMS, ":bool", Model),
    (f::MACRO_STEAL_PATTERNS, ":bool", Model),
    (f::MACRO_QUANTIZE, ":string", Model),
    (f::RACK_MACRO_DEVICE, "device", Model),
    (f::RACK_MACRO_INDEX, ":int", Model),
    (f::RACK_MACRO_KEY, ":string", Model),
    (f::RACK_MACRO_NAME, ":string", Model),
    (f::RACK_MACRO_VALUE, ":number", Live),
    (f::RACK_MACRO_BASE, ":number", Live),
    (f::RACK_MACRO_LOCKED, ":bool", Live),
    (f::RACK_MACRO_HAS_LOCKS, ":bool", Live),
    (f::RACK_MACRO_MAPPINGS, "(list-of macro-mapping)", Model),
    (f::MAPPING_MACRO, "macro", Model),
    (f::MAPPING_RACK_MACRO, "rack-macro", Model),
    (f::MAPPING_INDEX, ":int", Model),
    (f::MAPPING_TARGET, "param", Model),
    (f::MAPPING_LABEL, ":string", Model),
    (f::MAPPING_MIN, ":number", Model),
    (f::MAPPING_MAX, ":number", Model),
    (f::MAPPING_CURVE, ":string", Model),
    (f::MAPPING_SUSPENDED, ":bool", Model),
    (f::SCENE_INDEX, ":int", Model),
    (f::SCENE_NUMBER, ":int", Model),
    (f::SCENE_NAME, ":string", Model),
    (f::SCENE_ACTIVE, ":bool", Model),
    (f::SCENE_QUEUED, ":bool", Model),
    (f::SCENE_BANK, "bank", Model),
    (f::BANK_INDEX, ":int", Model),
    (f::BANK_LABEL, ":string", Model),
    (f::BANK_SCENES, "(list-of scene)", Model),
    (f::BANK_PLAYING, ":bool", Model),
    (f::BUS_INDEX, ":int", Model),
    (f::BUS_BID, ":int", Model),
    (f::BUS_NAME, ":string", Model),
    // Pushed every tick from the `App` (a handful of buses).
    (f::BUS_VOLUME, ":number", Model),
    (f::BUS_MUTED, ":bool", Model),
    (f::BUS_SOLOED, ":bool", Model),
    (f::BUS_PEAK, ":number", Live),
    (f::BUS_OUTPUT, "bus", Model),
    (f::BUS_OUTPUT_OPTIONS, "(list-of bus)", Model),
    (f::BUS_DEVICES, "(list-of device)", Model),
    (f::BUS_MOD_IN[0], ":number", Live),
    (f::BUS_MOD_IN[1], ":number", Live),
    (f::BUS_MOD_IN[2], ":number", Live),
    (f::BUS_MOD_IN[3], ":number", Live),
    (f::TUNING_TRACK, "track", Model),
    (f::TUNING_ON, ":bool", Model),
    (f::TUNING_SCALE, ":string", Model),
    (f::TUNING_CUSTOM, ":bool", Model),
    (f::TUNING_EDITED, ":bool", Model),
    (f::TUNING_ROOT, ":string", Model),
    (f::TUNING_MORPH, ":number", Model),
    (f::TUNING_MODE, ":string", Model),
    (f::TUNING_PERIOD, ":number", Model),
    (f::TUNING_DEGREES, "(list-of degree)", Model),
    (f::DEGREE_TUNING, "tuning", Model),
    (f::DEGREE_INDEX, ":int", Model),
    (f::DEGREE_BASE, ":number", Model),
    (f::DEGREE_OFFSET, ":number", Model),
    (f::DEGREE_ENABLED, ":bool", Model),
    (f::DEGREE_PITCH, ":number", Model),
    (f::DEGREE_LABEL, ":string", Model),
    (f::DEGREE_RATIO, ":string", Model),
    (f::ROUTE_INDEX, ":int", Model),
    (f::ROUTE_SOURCE, "track", Model),
    (f::ROUTE_DEST, "track", Model),
    (f::ROUTE_DEST_BUS, "bus", Model),
    (f::ROUTE_INPUT, ":int", Model),
    (f::ROUTE_SELECTED, ":bool", Live),
    (f::GROUP_INDEX, ":int", Model),
    (f::GROUP_GID, ":int", Model),
    (f::GROUP_NAME, ":string", Model),
    (f::GROUP_COLOR, ":rgb", Model),
    (f::GROUP_COLLAPSED, ":bool", Model),
    (f::GROUP_RACK, ":bool", Model),
    (f::GROUP_TRACKS, "(list-of track)", Model),
    (f::GROUP_BUS, "bus", Model),
    (f::GROUP_RACKS, "(list-of group)", Model),
    (f::GROUP_PARENT, "group", Model),
    (f::GROUP_ARMED, ":bool", Live),
    (f::GROUP_PADS, "(list-of pad)", Model),
    (f::GROUP_CLIPS, "(list-of rack-clip)", Model),
    (f::GROUP_RACK_CLIP, "rack-clip", Model),
    (f::GROUP_LEGACY, ":bool", Model),
    (f::GROUP_GROOVE, "groove", Model),
    (f::TRANSPORT_PLAYING, ":bool", Live),
    (f::TRANSPORT_RECORDING, ":bool", Live),
    (f::TRANSPORT_SCENE, "scene", Model),
    (f::TRANSPORT_QUEUED, "scene", Model),
    (f::TRANSPORT_LAUNCH_QUANTIZE, ":string", Model),
    (f::TRANSPORT_BPM, ":int", Live),
    (f::TRANSPORT_POSITION, ":int", Live),
    (f::TRANSPORT_METRONOME, ":bool", Live),
    (f::TRANSPORT_ROLL_MODE, ":bool", Live),
    (f::TRANSPORT_RECORD_QUANTIZE, ":string", Live),
    (f::TRANSPORT_ROLL_RATE, ":string", Live),
    (f::TRANSPORT_SEQUENCE_ROLLING, ":bool", Live),
    (f::MASTER_PEAK_L, ":number", Live),
    (f::MASTER_PEAK_R, ":number", Live),
    (f::MASTER_RECORDING, ":bool", Live),
    (f::ENGINE_CPU_LOAD, ":number", Live),
    (f::ENGINE_LATENCY_MS, ":number", Live),
    (f::ENGINE_OVERLOADED, ":bool", Live),
    // Compared every tick (the `App`'s pending compile moves no counter).
    (f::ENGINE_COMPILING, ":bool", Model),
    (f::SELECTION_TRACK, "track", Live),
    (f::SELECTION_TRACKS, "(list-of track)", Live),
    (f::SELECTION_STEPS, "(list-of step)", Live),
    (f::SELECTION_CURSOR_STEP, "step", Live),
    (f::SELECTION_EDIT_STEP, "step", Live),
    (f::SELECTION_RACK_SLOT, ":int", Model),
    (f::SELECTION_AUTO_FOLLOW, ":bool", Live),
    (f::PROJECT_TRACKS, "(list-of track)", Model),
    (f::PROJECT_SCENES, "(list-of scene)", Model),
    (f::PROJECT_BANKS, "(list-of bank)", Model),
    (f::PROJECT_BUSES, "(list-of bus)", Model),
    (f::PROJECT_GROUPS, "(list-of group)", Model),
    (f::PROJECT_ROUTES, "(list-of route)", Model),
    // The fixed lists once; the others when they change (`settings`).
    (f::PROJECT_FTS_OPTIONS, "(list-of :string)", Model),
    (f::PROJECT_SYNC_OPTIONS, "(list-of :string)", Model),
    (f::PROJECT_ACCUMULATOR_OPTIONS, "(list-of :string)", Model),
    (f::PROJECT_OUTPUT_OPTIONS, "(list-of bus)", Model),
    (f::PROJECT_STEP_PARAM_OPTIONS, "(list-of :string)", Model),
    (f::PROJECT_GROOVE_POOL, "(list-of pool-groove)", Model),
    (f::PROJECT_GROOVE_LIBRARY, "(list-of library-groove)", Model),
    (f::PROJECT_MACROS, "(list-of macro)", Model),
    // Process lanes (`lanes`): a track's processes registered on the first
    // read of `processes` or `lanes`, then synced behind the track's lane
    // key; the classes when the library's version moved.
    (f::TRACK_PROCESSES, "(list-of process)", Model),
    (f::TRACK_LANES, "(list-of lane)", Model),
    (f::CLASS_INDEX, ":int", Model),
    (f::CLASS_NAME, ":string", Model),
    (f::CLASS_DOC, ":string", Model),
    (f::CLASS_SOURCE_PATH, ":string", Model),
    (f::CLASS_TARGET, ":string", Model),
    (f::CLASS_LANE_COUNT, ":int", Model),
    (f::CLASS_PORTS, "(list-of :string)", Model),
    (f::LIBRARY_CLASSES, "(list-of process-class)", Model),
    (f::PROCESS_TRACK, "track", Model),
    (f::PROCESS_PROC_ID, ":int", Model),
    (f::PROCESS_INDEX, ":int", Model),
    (f::PROCESS_CLASS_REF, "process-class", Model),
    (f::PROCESS_CLASS_NAME, ":string", Model),
    (f::PROCESS_NAME, ":string", Model),
    (f::PROCESS_INSTANCE_NAME, ":string", Model),
    (f::PROCESS_PROJECT, ":bool", Model),
    (f::PROCESS_DEFAULT_LANE, ":bool", Model),
    (f::PROCESS_ROSTER, ":bool", Model),
    (f::PROCESS_ENABLED, ":bool", Model),
    (f::PROCESS_DOC, ":string", Model),
    (f::PROCESS_SOURCE_PATH, ":string", Model),
    (f::PROCESS_TARGET, ":string", Model),
    (f::PROCESS_LANES, "(list-of lane)", Model),
    (f::PROCESS_INLETS, "(list-of inlet)", Model),
    (f::PROCESS_PORTS, "(list-of port)", Model),
    (f::PROCESS_IN_PORTS, "(list-of :string)", Model),
    (f::PROCESS_CELLS, "(list-of state-cell)", Model),
    (f::PROCESS_EXPR, ":bool", Model),
    (f::PROCESS_EXPR_LINE, ":string", Model),
    (f::PROCESS_COMPILE_ERROR, ":string", Model),
    // The scheduler's run errors and scope histories: re-read while
    // observed, when their versions moved.
    (f::PROCESS_ERROR, ":string", Live),
    (f::LANE_PROCESS, "process", Model),
    (f::LANE_TRACK, "track", Model),
    (f::LANE_INDEX, ":int", Model),
    (f::LANE_POSITION, ":int", Model),
    (f::LANE_INLET, ":string", Model),
    (f::LANE_LABEL, ":string", Model),
    (f::LANE_SHORT_LABEL, ":string", Model),
    (f::LANE_TYPE, ":string", Model),
    (f::LANE_MIN, ":number", Model),
    (f::LANE_MAX, ":number", Model),
    (f::LANE_DEFAULT, ":number", Model),
    (f::LANE_DECIMALS, ":int", Model),
    (f::LANE_FORKED, ":bool", Model),
    (f::LANE_VALUES, "(list-of :number)", Model),
    (f::INLET_PROCESS, "process", Model),
    (f::INLET_INDEX, ":int", Model),
    (f::INLET_NAME, ":string", Model),
    (f::INLET_TYPE, ":string", Model),
    (f::INLET_OPTIONS, "(list-of :string)", Model),
    (f::INLET_VALUE, ":number", Model),
    (f::INLET_DEFAULT, ":number", Model),
    (f::INLET_MIN, ":number", Model),
    (f::INLET_MAX, ":number", Model),
    (f::INLET_DECIMALS, ":int", Model),
    (f::INLET_DOC, ":string", Model),
    (f::PORT_PROCESS, "process", Model),
    (f::PORT_INDEX, ":int", Model),
    (f::PORT_NAME, ":string", Model),
    (f::PORT_LABEL, ":string", Model),
    (f::PORT_HINT, ":string", Model),
    (f::PORT_TARGET, ":string", Model),
    (f::PORT_STATUS, ":string", Model),
    (f::PORT_MANUAL, ":bool", Model),
    (f::PORT_DISCONNECTED, ":bool", Model),
    (f::PORT_MAPPABLE, ":bool", Model),
    (f::PORT_CONNECTABLE, ":bool", Model),
    (f::PORT_BINDABLE, ":bool", Model),
    (f::PORT_TARGET_KIND, ":string", Model),
    (f::PORT_TARGET_PROCESS, "process", Model),
    (f::PORT_TARGET_INLET, ":string", Model),
    (f::PORT_TARGET_STEP_PARAM, ":string", Model),
    (f::PORT_FANOUT, "(list-of fanout)", Model),
    (f::FANOUT_PORT, "port", Model),
    (f::FANOUT_INDEX, ":int", Model),
    (f::FANOUT_TARGET, ":string", Model),
    (f::FANOUT_TARGET_PROCESS, "process", Model),
    (f::FANOUT_TARGET_INLET, ":string", Model),
    (f::FANOUT_TARGET_STEP_PARAM, ":string", Model),
    (f::FANOUT_LO, ":number", Model),
    (f::FANOUT_HI, ":number", Model),
    (f::STATE_CELL_PROCESS, "process", Model),
    (f::STATE_CELL_INDEX, ":int", Model),
    (f::STATE_CELL_NAME, ":string", Model),
    (f::STATE_CELL_VALUES, "(list-of :number)", Live),
    // The track's instrument for the browser, at the track model sync.
    (f::TRACK_INSTRUMENT_ID, ":string", Model),
    // The project's name, compared every tick; the audio worker choices
    // with the settings (`presented`).
    (f::PROJECT_NAME, ":string", Model),
    (f::PROJECT_AUDIO_WORKERS_OPTIONS, "(list-of :string)", Model),
    // The browser, the sound palette, the editor and the app's views
    // (`presented`): pushed from what the legacy publishers record
    // (`ui::presented`), each area when its generation moved; the sample
    // preview and the capture audition live.
    (f::BROWSER_TRACK, "track", Model),
    (f::BROWSER_INSTRUMENT_KIND, ":string", Model),
    (f::BROWSER_INSTRUMENT, ":string", Model),
    (f::BROWSER_INSTRUMENT_LABEL, ":string", Model),
    (f::BROWSER_PRESET, ":string", Model),
    (f::BROWSER_PRESETS, "(list-of :string)", Model),
    (f::BROWSER_USER_PRESETS, "(list-of :string)", Model),
    (f::BROWSER_SAMPLE, ":string", Model),
    (f::BROWSER_SLOTS, "(list-of slot-presets)", Model),
    (f::BROWSER_ENGINES, "(list-of :string)", Model),
    (f::BROWSER_SOUND_PRESETS, "(list-of preset-file)", Model),
    (f::BROWSER_KIT_PRESETS, "(list-of preset-file)", Model),
    (f::BROWSER_LIBRARY_EPOCH, ":int", Model),
    (f::BROWSER_PREVIEW_PLAYING, ":bool", Live),
    (f::BROWSER_PREVIEW_POSITION, ":number", Live),
    (f::PRESET_FILE_INDEX, ":int", Model),
    (f::PRESET_FILE_TYPE, ":string", Model),
    (f::PRESET_FILE_ICON, ":string", Model),
    (f::PRESET_FILE_NAME, ":string", Model),
    (f::PRESET_FILE_PATH, ":string", Model),
    (f::PRESET_FILE_PADS, ":int", Model),
    (f::PRESET_FILE_AUTHOR, ":string", Model),
    (f::PRESET_FILE_TAGS, "(list-of :string)", Model),
    (f::SLOT_PRESETS_INDEX, ":int", Model),
    (f::SLOT_PRESETS_DEVICE, "device", Model),
    (f::SLOT_PRESETS_INSTRUMENT, ":string", Model),
    (f::SLOT_PRESETS_INSTRUMENT_LABEL, ":string", Model),
    (f::SLOT_PRESETS_PRESETS, "(list-of :string)", Model),
    (f::SLOT_PRESETS_USER_PRESETS, "(list-of :string)", Model),
    (f::SLOT_PRESETS_PRESET, ":string", Model),
    (f::SOUND_TRACK, "track", Model),
    (f::SOUND_PATCH_ID, ":int", Model),
    (f::SOUND_MIX_ID, ":int", Model),
    (f::SOUND_NAME, ":string", Model),
    (f::SOUND_REFERENTS, ":string", Model),
    (f::SOUND_REFERENTS_SHORT, ":string", Model),
    (f::SOUND_BASE, ":bool", Model),
    (f::SOUND_TRACK_SOUND, ":bool", Model),
    (f::SOUND_CURRENT, ":bool", Model),
    (f::SOUND_PRESET, ":string", Model),
    (f::SOUND_SAMPLE, ":string", Model),
    (f::SOUND_DIFF_UP, ":int", Model),
    (f::SOUND_DIFF_DOWN, ":int", Model),
    (f::SOUND_COLORED, ":bool", Model),
    (f::SOUND_COLOR, ":rgb", Model),
    (f::SOUND_GLYPH_KEY, ":string", Model),
    (f::PALETTE_OPEN, ":bool", Model),
    (f::PALETTE_TRACK, "track", Model),
    (f::PALETTE_TARGET, ":string", Model),
    (f::PALETTE_TARGET_ID, ":int", Model),
    (f::PALETTE_INSTRUMENT, ":string", Model),
    (f::PALETTE_SOUNDS, "(list-of sound)", Model),
    (f::EDITOR_MODE, ":string", Model),
    (f::EDITOR_SURFACE, ":string", Model),
    (f::EDITOR_BUFFER, ":string", Model),
    (f::EDITOR_ERROR, ":string", Model),
    (f::EDITOR_CANCELING, ":bool", Model),
    (f::EDITOR_RUN_MODE, ":string", Model),
    (f::EDITOR_ACTIVE_MACRO, ":string", Model),
    (f::EDITOR_ACTIVE_MACRO_ACTION, ":string", Model),
    (f::EDITOR_OPEN_MACRO, ":string", Model),
    (f::EDITOR_PATCH_MACROS, "(list-of editor-macro)", Model),
    (f::EDITOR_LIBRARY_MACROS, "(list-of editor-macro)", Model),
    (f::EDITOR_ASSETS, "(list-of editor-asset)", Model),
    (f::EDITOR_SELECTED_ASSET, "asset-info", Model),
    (f::EDITOR_MACRO_NAME, ":string", Model),
    (f::EDITOR_MACRO_LIBRARY, ":bool", Model),
    (f::EDITOR_MACRO_PARAMS, "(list-of :string)", Model),
    (f::EDITOR_MACRO_CALLS, "(list-of :string)", Model),
    (f::EDITOR_MACRO_OUTPUTS, "(list-of :string)", Model),
    (f::EDITOR_MACRO_SUMMARY, ":string", Model),
    (f::EDITOR_MACRO_USED, ":bool", Model),
    (f::EDITOR_ASSET_INDEX, ":int", Model),
    (f::EDITOR_ASSET_REFERENCE, ":string", Model),
    (f::EDITOR_ASSET_TIER, ":string", Model),
    (f::EDITOR_ASSET_SOURCE_PATH, ":string", Model),
    (f::ASSET_REFERENCE, ":string", Model),
    (f::ASSET_TENSOR_KIND, ":string", Model),
    (f::ASSET_LAYOUT, ":string", Model),
    (f::ASSET_SHAPE, "(list-of :int)", Model),
    (f::ASSET_SOURCE, ":string", Model),
    (f::ASSET_WAVE_COUNT, ":int", Model),
    (f::ASSET_WAVES_PER_SET, ":int", Model),
    (f::ASSET_SET_COUNT, ":int", Model),
    (f::ASSET_SETS, "(list-of :string)", Model),
    (f::ASSET_WAVE_NAMES, "(list-of :string)", Model),
    (f::LEARN_TARGET_PATH, ":string", Model),
    (f::LEARN_TARGET_NAME, ":string", Model),
    (f::LEARN_PHASE, ":string", Model),
    (f::LEARN_METHOD, ":string", Model),
    (f::LEARN_EPOCHS, ":int", Model),
    (f::LEARN_CMA_GENERATIONS, ":int", Model),
    (f::LEARN_CMA_POPULATION, ":int", Model),
    (f::LEARN_CMA_SIGMA, ":number", Model),
    (f::LEARN_CMA_SEED, ":int", Model),
    (f::LEARN_CMA_FORWARD_BATCH, ":int", Model),
    (f::LEARN_LOCAL_EPOCHS, ":int", Model),
    (f::LEARN_CMA_CONTINUE, ":int", Model),
    (f::LEARN_CMA_REFINE_EPOCHS, ":int", Model),
    (f::LEARN_CMA_REFINE_MODE, ":string", Model),
    (f::LEARN_CMA_FINAL_EPOCHS, ":int", Model),
    (f::LEARN_PITCH_HZ, ":number", Model),
    (f::LEARN_GATE_FRAMES, ":int", Model),
    (f::LEARN_STAGE, ":string", Model),
    (f::LEARN_CURRENT_EPOCH, ":int", Model),
    (f::LEARN_TOTAL_EPOCHS, ":int", Model),
    (f::LEARN_LOSS, ":number", Model),
    (f::LEARN_LOSSES, "(list-of :number)", Model),
    (f::LEARN_OPTIMIZATION_LOSSES, "(list-of :number)", Model),
    (f::LEARN_PLAN_PARAMS, "(list-of learn-plan-param)", Model),
    (f::LEARN_EPOCH_PARAMS, "(list-of learn-epoch-param)", Model),
    (f::LEARN_IMPROVEMENT_PCT, ":number", Model),
    (f::LEARN_ABS_DISTANCE, ":number", Model),
    (f::LEARN_BASIN_CHECK, ":string", Model),
    (f::LEARN_RESULT_DELTAS, "(list-of learn-delta)", Model),
    (f::LEARN_SEEDED_WAV, ":string", Model),
    (f::LEARN_FINAL_WAV, ":string", Model),
    (f::LEARN_APPLIED, ":bool", Model),
    (f::LEARN_ERROR, ":string", Model),
    (f::PLAN_PARAM_INDEX, ":int", Model),
    (f::PLAN_PARAM_NAME, ":string", Model),
    (f::PLAN_PARAM_STATUS, ":string", Model),
    (f::PLAN_PARAM_REASON, ":string", Model),
    (f::EPOCH_PARAM_INDEX, ":int", Model),
    (f::EPOCH_PARAM_NAME, ":string", Model),
    (f::EPOCH_PARAM_FROM, ":number", Model),
    (f::EPOCH_PARAM_VALUE, ":number", Model),
    (f::EPOCH_PARAM_CHANGE, ":number", Model),
    (f::EPOCH_PARAM_STEP, ":number", Model),
    (f::DELTA_INDEX, ":int", Model),
    (f::DELTA_NAME, ":string", Model),
    (f::DELTA_FROM, ":number", Model),
    (f::DELTA_TO, ":number", Model),
    (f::DELTA_CHANGE, ":number", Model),
    (f::RETRO_LANES, "(list-of retro-lane)", Model),
    (f::RETRO_ITEMS, "(list-of retro-item)", Model),
    (f::RETRO_DURATION, ":number", Model),
    (f::RETRO_TRUNCATED, ":bool", Model),
    (f::RETRO_ERROR, ":string", Model),
    (f::RETRO_PLAYING, ":bool", Live),
    (f::RETRO_POSITION, ":number", Live),
    (f::RETRO_LANE_INDEX, ":int", Model),
    (f::RETRO_LANE_LABEL, ":string", Model),
    (f::RETRO_ITEM_INDEX, ":int", Model),
    (f::RETRO_ITEM_LANE, "retro-lane", Model),
    (f::RETRO_ITEM_START, ":number", Model),
    (f::RETRO_ITEM_END, ":number", Model),
    (f::EXPORT_DEFAULT_NAME, ":string", Model),
    (f::EXPORT_PROJECT, ":string", Model),
    (f::EXPORT_FOLDER, ":string", Model),
    (f::EXPORT_END, ":number", Model),
    (f::EXPORT_BUSY, ":bool", Model),
    (f::EXPORT_DONE, ":bool", Model),
    (f::EXPORT_MESSAGE, ":string", Model),
    (f::EXPORT_PERCENT, ":number", Model),
    (f::EXPORT_OUTPUT_NAME, ":string", Model),
    (f::EXPORT_REVEAL_LABEL, ":string", Model),
    (f::MIDI_DEVICE_INDEX, ":int", Model),
    (f::MIDI_DEVICE_ID, ":string", Model),
    (f::MIDI_DEVICE_NAME, ":string", Model),
    (f::MIDI_DEVICE_ENABLED, ":bool", Model),
    (f::MIDI_DEVICE_CONNECTED, ":bool", Model),
    (f::MIDI_DEVICE_STATUS, ":string", Model),
    (f::SETTINGS_AUDIO_WORKERS_CHOICE, ":string", Model),
    (f::SETTINGS_AUDIO_WORKERS_NOTE, ":string", Model),
    (f::SETTINGS_MIDI_DEVICES, "(list-of midi-device)", Model),
    (f::SETTINGS_MIDI_ERROR, ":string", Model),
    (f::SETTINGS_MIDI_PERSISTENT, ":bool", Model),
    (f::AGENT_GENERATION, ":int", Model),
];

/// The published kinds, in [`PUBLISHED`] order.
static PUBLISHED_KINDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut kinds: Vec<&'static str> = Vec::new();
    for ((kind, _), _, _) in PUBLISHED {
        if !kinds.contains(kind) {
            kinds.push(kind);
        }
    }
    kinds
});

/// The kind names `eseq.kinds` reserves (spec §3.4): every published kind.
pub(crate) fn host_kind_names() -> Vec<&'static str> {
    PUBLISHED_KINDS
        .iter()
        .map(|kind| eseqlisp::vm::kind_name_of(kind))
        .collect()
}

/// The live fields of one kind, with their names for
/// `Runtime::host_fields_observed` (bit `i` is `keys[i]`).
pub(super) struct LiveFields {
    pub(super) keys: Vec<FieldKey>,
    pub(super) names: Vec<&'static str>,
}

impl LiveFields {
    fn of(kind: &str) -> Self {
        let keys: Vec<FieldKey> = PUBLISHED
            .iter()
            .filter(|((published, _), _, feed)| *published == kind && *feed == Live)
            .map(|(key, _, _)| *key)
            .collect();
        // Observed masks are `u32`s.
        assert!(keys.len() <= 32, "{kind} has more than 32 live fields");
        let names = keys.iter().map(|(_, name)| *name).collect();
        Self { keys, names }
    }

    /// The observed-mask bit of `key`.
    pub(super) fn bit(&self, key: FieldKey) -> u32 {
        let index = self.keys.iter().position(|live| *live == key);
        index.map_or(0, |index| 1 << index)
    }

    /// The observed-mask bits of `keys`.
    pub(super) fn bits(&self, keys: &[FieldKey]) -> u32 {
        keys.iter().fold(0, |bits, key| bits | self.bit(*key))
    }
}

pub(super) static TRACK_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(TRACK));
pub(super) static STEP_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(STEP));
pub(super) static TRANSPORT_LIVE: LazyLock<LiveFields> =
    LazyLock::new(|| LiveFields::of(TRANSPORT));
pub(super) static SELECTION_LIVE: LazyLock<LiveFields> =
    LazyLock::new(|| LiveFields::of(SELECTION));
pub(super) static SEND_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(SEND));
pub(super) static BUS_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(BUS));
pub(super) static MASTER_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(MASTER));
pub(super) static ENGINE_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(ENGINE));
pub(super) static PARAM_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(PARAM));
pub(super) static ROUTE_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(ROUTE));
pub(super) static CELL_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(CELL));
pub(super) static SONG_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(SONG));
pub(super) static GROUP_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(GROUP));
pub(super) static PAD_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(PAD));
pub(super) static DEVICE_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(DEVICE));
pub(super) static TENSOR_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(TENSOR));
pub(super) static VARIANT_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(VARIANT));
pub(super) static RACK_MACRO_LIVE: LazyLock<LiveFields> =
    LazyLock::new(|| LiveFields::of(RACK_MACRO));
pub(super) static PROCESS_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(PROCESS));
pub(super) static STATE_CELL_LIVE: LazyLock<LiveFields> =
    LazyLock::new(|| LiveFields::of(STATE_CELL));
pub(super) static BROWSER_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(BROWSER));
pub(super) static RETRO_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(RETRO));

/// The step fields diffed by value per tick (beside `active`, `selected`
/// and `playing`): `held`, then the step parameters, whose field names are
/// their `seq-set-step-param` keywords ([`step_param_named`]).
pub(super) const STEP_VALUES: [FieldKey; 10] = [
    f::STEP_HELD,
    f::STEP_VELOCITY,
    f::STEP_DURATION,
    f::STEP_TRANSPOSE,
    f::STEP_DELAY,
    f::STEP_RETRIG,
    f::STEP_RETRIG_RATE,
    f::STEP_PAN,
    f::STEP_SYNC,
    f::STEP_AUX_A,
];

/// Every live field.
pub(super) static LIVE_KEYS: LazyLock<Vec<FieldKey>> = LazyLock::new(|| {
    PUBLISHED
        .iter()
        .filter(|(_, _, feed)| *feed == Live)
        .map(|(key, _, _)| *key)
        .collect()
});

/// One way the loaded `eseq.kinds` differs from [`PUBLISHED`].
struct Mismatch {
    message: String,
    /// The published fields the host must not push while it stands.
    skip: Vec<FieldKey>,
}

fn schema_mismatches(rt: &Runtime) -> Vec<Mismatch> {
    let mut mismatches = Vec::new();
    for kind in PUBLISHED_KINDS.iter().copied() {
        let fields = || {
            PUBLISHED
                .iter()
                .filter(move |((published, _), _, _)| *published == kind)
        };
        let Some(schema) = rt.instance_kind_schema(kind) else {
            mismatches.push(Mismatch {
                message: format!("{KINDS_MODULE}: kind '{kind}' is not declared"),
                skip: fields().map(|(key, _, _)| *key).collect(),
            });
            continue;
        };
        for (key, ty, _) in fields() {
            let field = key.1;
            let message = match schema
                .host
                .iter()
                .find(|declared| declared.field.name == field)
            {
                None => format!(
                    "{kind}: the host publishes '{field}' ({ty}), which is not a :host field"
                ),
                Some(declared) if declared.field.ty.to_string() != *ty => format!(
                    "{kind}: '{field}' is declared {}, the host publishes {ty}",
                    declared.field.ty
                ),
                Some(_) => continue,
            };
            mismatches.push(Mismatch {
                message,
                skip: vec![*key],
            });
        }
        for declared in &schema.host {
            if !fields().any(|(key, _, _)| key.1 == declared.field.name) {
                mismatches.push(Mismatch {
                    message: format!(
                        "{kind}: :host field '{}' is declared but the host never publishes it",
                        declared.field.name
                    ),
                    skip: Vec::new(),
                });
            }
        }
    }
    mismatches
}

/// Check the loaded `eseq.kinds` against [`PUBLISHED`]: every published
/// field is a declared `:host` field of that type, and every declared
/// `:host` field is published. Returns one message per mismatch.
pub(crate) fn check_schema(rt: &Runtime) -> Result<(), Vec<String>> {
    let errors: Vec<String> = schema_mismatches(rt)
        .into_iter()
        .map(|mismatch| mismatch.message)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn schema_message(errors: &[String]) -> String {
    format!(
        "host kinds do not match {KINDS_MODULE} (content/core/modules/kinds.lisp):\n  {}",
        errors.join("\n  ")
    )
}

/// Run [`check_schema`] at startup: a hard error in debug builds, a
/// warning in release (spec §3.4). After startup the tick re-checks on
/// every schema change and only warns ([`HostKinds::sync`]).
pub(crate) fn check_schema_at_startup(rt: &Runtime) {
    if let Err(errors) = check_schema(rt) {
        let message = schema_message(&errors);
        if cfg!(debug_assertions) {
            panic!("{message}");
        }
        eprintln!("metal_seq: warning: {message}");
    }
}

/// Reserve the host kind names for `eseq.kinds` (spec §3.4).
pub(crate) fn reserve_kind_names(rt: &mut Runtime) {
    rt.reserve_kind_names(KINDS_MODULE, &host_kind_names());
}

fn number(n: impl Into<f64>) -> Value {
    Value::Number(n.into())
}

fn text(text: &str) -> Value {
    Value::String(text.to_string())
}

fn instance_or_nil(id: Option<InstanceId>) -> Value {
    id.map_or(Value::Nil, Value::Instance)
}

fn instance_list(ids: impl IntoIterator<Item = InstanceId>) -> Value {
    list_value(ids.into_iter().map(Value::Instance))
}

/// The instances of a reconciled listing (the failed ones left out).
fn listed_instances(ids: &[Option<InstanceId>]) -> Value {
    instance_list(ids.iter().flatten().copied())
}

pub(super) fn strings<'a>(items: impl IntoIterator<Item = &'a String>) -> Value {
    list_value(items.into_iter().map(|item| text(item)))
}

fn rgb3([r, g, b]: [f32; 3]) -> Value {
    eseqlisp::vm::tagged_list("rgb", vec![number(r), number(g), number(b)])
}

fn rgb(color: sequencer::track_color::TrackColor) -> Value {
    rgb3([color.r, color.g, color.b])
}

// ── the tick ────────────────────────────────────────────────────────────

/// The change counters the model fields derive from; the model half of a
/// sync runs only when this moves (or the track order does). Mirrors
/// `capture_param_sync_revision` (reactive_tick.rs).
#[derive(Clone, PartialEq)]
struct ModelRevision {
    ui_epoch: usize,
    fx_epoch: usize,
    fx_value_epoch: usize,
    pattern_epoch: u64,
    song_row_mirror_epoch: u64,
    sound_binding_epoch: usize,
    history_revision: u64,
    scenes_revision: u64,
    current_scene: usize,
    tracks: usize,
    active_tracks: usize,
    track_generation: u64,
    /// The device registry's generation: a bind can give a chain device an
    /// identity (its `did`) and move no other counter, and the track chain
    /// sync must then re-key the device.
    device_registry: u64,
    /// The display tint and palette track colors go through (a theme change
    /// bumps no epoch).
    track_tint: ThemeTint,
    /// The same for p-lock variant and sound palette colors (the sound
    /// palette's sync compares it; read once per tick here).
    variant_tint: ThemeTint,
}

/// A tint and the palette colors go through it (`eseqlisp::theme`'s display
/// keys).
pub(super) type ThemeTint = (
    eseqlisp::backend::Color,
    [eseqlisp::backend::Color; eseqlisp::theme::TRACK_PALETTE_SLOTS],
);

impl ModelRevision {
    fn capture(app: &app::App, shared: &KindsHandles) -> Self {
        Self {
            ui_epoch: shared.ui_epoch.load(Ordering::Relaxed),
            fx_epoch: shared.fx_epoch.load(Ordering::Relaxed),
            fx_value_epoch: shared.fx_value_epoch.load(Ordering::Relaxed),
            pattern_epoch: app.state.transport.pattern_epoch.load(Ordering::Relaxed),
            song_row_mirror_epoch: app.song_row_mirror_epoch,
            sound_binding_epoch: app.sound_binding_epoch,
            history_revision: app.history.current_revision(),
            scenes_revision: app.state.project_scenes_revision(),
            current_scene: app.state.current_scene_index(),
            tracks: app.tracks.len(),
            active_tracks: app.state.active_track_count(),
            track_generation: app.track_registry.generation(),
            device_registry: app.device_registry.generation(),
            track_tint: eseqlisp::theme::track_display_key(),
            variant_tint: eseqlisp::theme::variant_display_key(),
        }
    }
}

/// The host side of `eseq.kinds`, kept across ticks (in
/// `FrameDiffState`).
#[derive(Default)]
pub(crate) struct HostKinds {
    pub(crate) shared: Rc<RefCell<KindsShared>>,
    /// Built when the reader is installed.
    sources: Option<Rc<KindsHandles>>,
    /// The schema generation [`check_schema`] last ran against, and the
    /// mismatches it warned about.
    schema_generation: Option<u64>,
    warned: Vec<String>,
    /// The model revision of the last model sync; `None` forces one.
    model: Option<ModelRevision>,
    /// The track registry's order at the last model sync.
    model_track_ids: Vec<sequencer::sequencer::TrackId>,
    /// `TrackId` → instance, for the registry generation in
    /// `track_generation`, so a reorder re-keys rather than re-registers
    /// (spec §4) and a project load replaces every track (and bus and
    /// group).
    tracks: HashMap<u64, InstanceId>,
    track_generation: Option<u64>,
    /// Scene / bank / bus / group id → instance.
    scenes: HashMap<u64, InstanceId>,
    banks: HashMap<u64, InstanceId>,
    buses: HashMap<u64, InstanceId>,
    groups: HashMap<u64, InstanceId>,
    /// The bus ids and groups of the last model sync: a bus added or
    /// removed, or any group edit (collapse included), forces one.
    model_bus_ids: Vec<u64>,
    model_groups: Vec<sequencer::project::ProjectTrackGroup>,
    /// Moved whenever the model sync records new `model_groups` (the rack
    /// sync's key, so it need not compare the groups itself).
    groups_generation: u64,
    /// The instances at each position as of the last model sync.
    track_ids: Vec<Option<InstanceId>>,
    scene_ids: Vec<Option<InstanceId>>,
    bank_ids: Vec<Option<InstanceId>>,
    bus_ids: Vec<Option<InstanceId>>,
    group_ids: Vec<Option<InstanceId>>,
    /// Every send instance, for the live loop.
    send_ids: Vec<InstanceId>,
    /// Every device instance, for the live loop (its params are its
    /// children).
    device_ids: Vec<InstanceId>,
    /// The observed devices and params, kept per observer epoch and reset
    /// when their instances change.
    device_observed: ObservedList,
    param_observed: ObservedList,
    /// The union of the send (bus) instances' observed live fields, as of
    /// `Runtime::instance_observer_epoch` ([`observed_union`]).
    send_observers: Option<(u64, u32)>,
    bus_observers: Option<(u64, u32)>,
    /// Bus (volume, mute, solo) last pushed, by bus position.
    bus_mixer: Vec<Option<(f32, bool, bool)>>,
    /// `selection.tracks` (sorted track positions) last pushed; `None`
    /// while unobserved.
    selection_tracks: Option<Vec<usize>>,
    steps: HashMap<InstanceId, StepDiff>,
    selection: StepSelection,
    /// Per-step changed-field masks, reused across tracks and ticks.
    step_changes: Vec<u32>,
    /// The transport's queued scene and launch quantization last pushed.
    queued: Option<Option<usize>>,
    launch_quantize: Option<String>,
    /// Whether any track's `peak`, any bus's `peak`, or a master peak
    /// was observed at the last sync.
    peaks_observed: bool,
    bus_peaks_observed: bool,
    master_peaks_observed: bool,
    /// Whether a track's or a bus's mod port level was observed at the
    /// last sync.
    track_mod_levels_observed: bool,
    bus_mod_levels_observed: bool,
    /// Each track's settings as last pushed ([`TrackSettings`]), each
    /// tuning instance's scale and tuning as last pushed, and the project's
    /// option lists.
    settings: HashMap<InstanceId, TrackSettings>,
    tunings: HashMap<InstanceId, (usize, sequencer::scale::TrackTuning)>,
    project_options: ProjectOptions,
    /// `t.bar-transposes` last pushed, per observing track.
    bar_transposes: HashMap<InstanceId, Vec<f64>>,
    /// Route id → instance; the ids are allocated per [`RouteKey`] while
    /// the route exists (never reused), and forgotten on a project load
    /// (track ids restart).
    routes: HashMap<u64, InstanceId>,
    route_keys: HashMap<RouteKey, u64>,
    next_route_id: u64,
    route_ids: Vec<Option<InstanceId>>,
    route_observed: ObservedList,
    /// The observed groups (`armed`), reset when the group instances move.
    group_observed: ObservedList,
    /// `engine.compiling` last pushed.
    compiling: Option<bool>,
    /// The current track `selection.rack-slot` was last computed for.
    rack_slot_track: Option<usize>,
    /// (current track, its length, the step cursor) when `selection`'s
    /// step fields were last pushed; `None` forces a push.
    selection_cursor: Option<(usize, usize, usize)>,
    /// The song, its clips and scene spans, and the tracks' cells.
    song: SongState,
    /// The drum racks' pads, clips and grooves, the groove pool and library.
    racks: RackState,
    /// Devices beyond the track chain: MIDI effects, bus effects, drum rack
    /// slots and their effects.
    pub(crate) devices: DeviceState,
    /// The device panel extras: tensors, variants, the modulation sample.
    panel: PanelState,
    /// Project and drum rack macros.
    pub(crate) macros: MacroState,
    /// Process lanes: the library's classes, the tracks' processes.
    pub(crate) lanes: LaneState,
    /// The browser, the sound palette, the editor and the app's views.
    pub(crate) presented: PresentedState,
}

impl HostKinds {
    /// Whether any track's `peak` was observed at the last sync: the tick
    /// then keeps the track meter cache polled even with no legacy meter
    /// on screen.
    pub(crate) fn wants_peaks(&self) -> bool {
        self.peaks_observed
    }

    /// Like [`Self::wants_peaks`], for the bus meters.
    pub(crate) fn wants_bus_peaks(&self) -> bool {
        self.bus_peaks_observed
    }

    /// Like [`Self::wants_peaks`], for the master meter.
    pub(crate) fn wants_master_peaks(&self) -> bool {
        self.master_peaks_observed
    }

    /// Like [`Self::wants_peaks`], for the mod port levels.
    pub(crate) fn wants_mod_levels(&self) -> bool {
        self.track_mod_levels_observed || self.bus_mod_levels_observed
    }

    /// One sync: schema check (on change), registry and model fields (on a
    /// model revision change), queued scene and quantization, observed live
    /// fields. `meters` is the tick's meter cache.
    /// Returns whether anything changed (the reactive cycle has then run).
    pub(crate) fn sync(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        shared: &SharedHandles,
        meters: &KindsMeters<'_>,
    ) -> bool {
        match &self.sources {
            Some(sources) => self.sync_sources(app, rt, sources.clone(), meters),
            None => self.sync_with(app, rt, &KindsHandles::of(shared), meters),
        }
    }

    /// [`Self::sync`] over explicit handles (headless capture).
    pub(crate) fn sync_with(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        handles: &KindsHandles,
        meters: &KindsMeters<'_>,
    ) -> bool {
        let sources = match &self.sources {
            Some(sources) => sources.clone(),
            None => Rc::new(handles.clone()),
        };
        self.sync_sources(app, rt, sources, meters)
    }

    fn sync_sources(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        sources: Rc<KindsHandles>,
        meters: &KindsMeters<'_>,
    ) -> bool {
        if rt.instance_kind_schema(TRACK).is_none() {
            return false; // eseq.kinds is not loaded
        }
        if self.refresh_schema(rt) {
            self.model = None;
            self.reset_project_options();
            self.song.invalidate();
            self.racks.invalidate();
            self.devices.invalidate();
            self.macros.invalidate();
            self.lanes.invalidate(&self.shared);
            self.presented.invalidate();
        }
        if self
            .song
            .representatives()
            .any(|id| !rt.instance_is_live(*id))
        {
            // A hot reload dropped arrangement instances.
            self.song.invalidate();
        }
        if (self.racks.representatives()).any(|id| !rt.instance_is_live(*id)) {
            // A hot reload dropped drum rack instances.
            self.racks.invalidate();
        }
        if self.sources.is_none() {
            install_reader(rt, sources.clone(), self.shared.clone());
            self.sources = Some(sources.clone());
            self.model = None;
        }
        let mod_display = self.panel.mod_display_observed;
        self.shared
            .borrow_mut()
            .copy_meters(app, meters, mod_display);
        self.shared.borrow_mut().capture_head = app.pending_capture_head_beat();
        let shared_kinds = self.shared.clone();
        let mut pusher = Pusher {
            rt,
            sources: &sources,
            shared: &shared_kinds,
            changed: false,
        };
        let revision = ModelRevision::capture(app, &sources);
        let variant_tint = revision.variant_tint;
        let groups_moved = app.groups != self.model_groups;
        let model_due = self.model.as_ref() != Some(&revision)
            || app.track_registry.ids() != self.model_track_ids.as_slice()
            || !app
                .buses
                .iter()
                .map(|bus| bus.id.0)
                .eq(self.model_bus_ids.iter().copied())
            || groups_moved
            || self.cached_instances_stale(pusher.rt);
        if model_due {
            self.launch_quantize = None;
            self.send_observers = None;
            self.bus_observers = None;
            self.bus_mixer.clear();
            self.selection_tracks = None;
            self.selection_cursor = None;
            self.rack_slot_track = None;
            self.compiling = None;
            self.replace_on_project_load(&mut pusher, app);
            let buses_done = self.sync_bus_model(&mut pusher, app);
            let tracks_done = self.sync_track_model(&mut pusher, app);
            let scenes_done = self.sync_scene_model(&mut pusher, app);
            self.shared.borrow_mut().model_syncs += 1;
            if buses_done && tracks_done && scenes_done {
                self.model = Some(revision);
                self.model_track_ids.clear();
                self.model_track_ids
                    .extend_from_slice(app.track_registry.ids());
                self.model_bus_ids.clear();
                self.model_bus_ids
                    .extend(app.buses.iter().map(|bus| bus.id.0));
                if groups_moved {
                    self.model_groups.clone_from(&app.groups);
                    self.groups_generation += 1;
                }
            } else {
                // Ids were unavailable this frame: try again next tick.
                self.model = None;
            }
        }
        self.sync_device_model(&mut pusher, app);
        self.refresh_sampler_playheads(&mut pusher, app);
        self.sync_macro_model(&mut pusher, app);
        self.sync_lane_model(&mut pusher);
        self.sync_cell_model(&mut pusher, app);
        self.sync_rack_clips(&mut pusher, app);
        self.sync_rack_model(&mut pusher, app);
        self.sync_groove_library(&mut pusher, app);
        self.sync_song_model(&mut pusher, app);
        self.sync_song_pushed(&mut pusher, app);
        self.sync_governed(&mut pusher, app);
        self.sync_presented(&mut pusher, app, &variant_tint);
        self.sync_transport_queue(&mut pusher, app);
        self.sync_bus_mixer(&mut pusher, app);
        self.sync_compiling(&mut pusher, app);
        self.sync_rack_slot(&mut pusher, app);
        let selection_changed = self.selection.refresh(&sources);
        self.sync_track_live(&mut pusher, selection_changed);
        self.sync_send_live(&mut pusher);
        self.sync_device_live(&mut pusher);
        self.sync_tensor_live(&mut pusher);
        self.sync_variant_live(&mut pusher);
        self.sync_rack_macro_live(&mut pusher);
        self.sync_lane_live(&mut pusher);
        self.sync_bus_live(&mut pusher);
        self.sync_route_live(&mut pusher);
        self.sync_group_live(&mut pusher);
        self.sync_cell_live(&mut pusher);
        self.sync_rack_live(&mut pusher);
        let singletons = [
            (TRANSPORT, &*TRANSPORT_LIVE),
            (ENGINE, &*ENGINE_LIVE),
            (SONG, &*SONG_LIVE),
            (BROWSER, &*BROWSER_LIVE),
            (RETRO, &*RETRO_LIVE),
        ];
        for (kind, fields) in singletons {
            if let Some(id) = pusher.singleton(kind) {
                pusher.push_live(id, fields);
            }
        }
        let master_peaks = MASTER_LIVE.bit(f::MASTER_PEAK_L) | MASTER_LIVE.bit(f::MASTER_PEAK_R);
        self.master_peaks_observed = pusher
            .singleton(MASTER)
            .is_some_and(|id| pusher.push_live(id, &MASTER_LIVE) & master_peaks != 0);
        self.sync_selection(&mut pusher, selection_changed);
        let changed = pusher.changed;
        if changed {
            rt.run_reactive_cycle();
        }
        changed
    }

    /// Re-run [`check_schema`] when a kind schema changed (a hot reload of
    /// `eseq.kinds`): warn once per distinct mismatch set and skip the
    /// mismatched fields until fixed. Returns whether the generation moved.
    fn refresh_schema(&mut self, rt: &Runtime) -> bool {
        let generation = rt.instance_kind_schema_generation();
        if self.schema_generation == Some(generation) {
            return false;
        }
        self.schema_generation = Some(generation);
        let mismatches = schema_mismatches(rt);
        let messages: Vec<String> = mismatches.iter().map(|m| m.message.clone()).collect();
        if !messages.is_empty() && messages != self.warned {
            eprintln!(
                "metal_seq: warning: {}\n  (skipping those fields until fixed)",
                schema_message(&messages)
            );
        }
        self.warned = messages;
        self.shared.borrow_mut().skip = mismatches
            .into_iter()
            .flat_map(|mismatch| mismatch.skip)
            .collect();
        true
    }

    /// Whether an instance the last model sync produced is gone (a hot
    /// reload dropped it): the model sync must run again.
    fn cached_instances_stale(&self, rt: &Runtime) -> bool {
        self.track_ids
            .iter()
            .chain(&self.scene_ids)
            .chain(&self.bank_ids)
            .chain(&self.bus_ids)
            .chain(&self.group_ids)
            .chain(&self.route_ids)
            .flatten()
            .any(|id| !rt.instance_is_live(*id))
    }

    /// A new track registry generation (a project load or clear): track,
    /// bus, group and pool groove ids restart with the project, so they
    /// name other things now. Drops every such instance (a group's pads,
    /// rack clips and grooves with it), once per generation, so the
    /// syncs after it re-register them (and keep the new ones across ticks
    /// while the registry lags the track list).
    fn replace_on_project_load(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let generation = app.track_registry.generation();
        if self.track_generation == Some(generation) {
            return;
        }
        let doomed = self
            .tracks
            .drain()
            .chain(self.buses.drain())
            .chain(self.groups.drain())
            .chain(self.routes.drain())
            .chain(self.racks.pool.drain())
            .chain(self.macros.drain())
            .chain(self.lanes.drain());
        for (_, id) in doomed {
            pusher.rt.drop_instance(id);
            pusher.changed = true;
        }
        self.route_keys.clear();
        self.track_generation = Some(generation);
    }

    /// `selection.track` and `auto-follow` when observed; `selection.tracks`
    /// when observed and the sorted selection changed since the last push;
    /// the step fields (`steps`, `cursor-step`, `edit-step`) when observed
    /// and the step selection (`selection_changed`), the current track, its
    /// length or the step cursor moved.
    fn sync_selection(&mut self, pusher: &mut Pusher<'_>, selection_changed: bool) {
        let Some(id) = pusher.singleton(SELECTION) else {
            return;
        };
        let mask = pusher.rt.host_fields_observed(id, &SELECTION_LIVE.names);
        let always = SELECTION_LIVE.bits(&[f::SELECTION_TRACK, f::SELECTION_AUTO_FOLLOW]);
        pusher.push_live_masked(id, &SELECTION_LIVE, mask & always);
        let step_bits = SELECTION_LIVE.bits(&[
            f::SELECTION_STEPS,
            f::SELECTION_CURSOR_STEP,
            f::SELECTION_EDIT_STEP,
        ]);
        if mask & step_bits == 0 {
            self.selection_cursor = None;
        } else {
            let sources = pusher.sources;
            let current = sources.current_track.load(Ordering::Relaxed);
            let num_steps = if sources.track_exists(current) {
                sources.num_steps(current)
            } else {
                0
            };
            let cursor = fx_step_cursor_value(pusher.rt.global_value(FX_STEP_CURSOR_GLOBAL));
            let key = Some((current, num_steps, cursor));
            if selection_changed || self.selection_cursor != key {
                self.selection_cursor = key;
                pusher.push_live_masked(id, &SELECTION_LIVE, mask & step_bits);
            }
        }
        if mask & SELECTION_LIVE.bit(f::SELECTION_TRACKS) == 0 {
            self.selection_tracks = None;
            return;
        }
        let tracks = sorted_selected_tracks(&pusher.sources.selected_tracks.lock().unwrap());
        if self.selection_tracks.as_ref() != Some(&tracks) {
            pusher.push_live_field(id, f::SELECTION_TRACKS);
            self.selection_tracks = Some(tracks);
        }
    }
}

impl HostKinds {
    /// `engine.compiling`, compared every tick: a compile starts and lands
    /// without moving a model counter.
    fn sync_compiling(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let compiling = app.compile_pending();
        if self.compiling == Some(compiling) {
            return;
        }
        if let Some(engine) = pusher.singleton(ENGINE) {
            pusher.push(engine, f::ENGINE_COMPILING, Value::Bool(compiling));
            self.compiling = Some(compiling);
        }
    }

    /// `selection.rack-slot`: the current drum rack's selected slot (-1
    /// when the current track is no rack), from
    /// the `App`; re-derived at the model sync (selecting a slot moves the
    /// UI epoch) and when the current track changes.
    fn sync_rack_slot(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let current = pusher.sources.current_track.load(Ordering::Relaxed);
        if self.rack_slot_track == Some(current) {
            return;
        }
        let Some(selection) = pusher.singleton(SELECTION) else {
            return;
        };
        let slot = rack_slot_selection(app, current).map_or(-1.0, |(slot, _)| slot as f64);
        pusher.push(selection, f::SELECTION_RACK_SLOT, number(slot));
        self.rack_slot_track = Some(current);
    }
}

#[cfg(test)]
mod tests;
