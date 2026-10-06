//! The browser, the sound palette, the editor and the app's views (spec
//! §14.2i, stage 7f): `browser` with its `preset-file`s and `slot-presets`,
//! `sound-palette` and its `sound`s, `editor` with its `editor-macro`s,
//! `editor-asset`s and `asset-info`, `learn` with its `learn-plan-param`,
//! `learn-epoch-param` and `learn-delta` rows, `retro` with its
//! `retro-lane`s and `retro-item`s, `song-export`, `settings` with its
//! `midi-device`s, `agent`, and `project.name` /
//! `project.audio-workers-options`.
//!
//! **Feeds.** The model fields come from the presented record
//! (`ui::presented`, which commands, job events and the snapshot
//! publishers edit): one area per view (the sidebar, the Sound and kit
//! listings, the palette, the editor, its macro sidebar, Patch Learn, the
//! capture, the export, the settings, the agent), each pushed only when its
//! generation moved since the last push. An idle tick compares those
//! counters and allocates nothing, and nothing here lists a directory or
//! reads a file: the listings are the ones the legacy publishers made. Also
//! compared every tick, in place: the sidebar's track and slot devices
//! against the track and device instances, the palette's track and the
//! variant tint its colors go through (read once per tick, in the
//! `ModelRevision`), the project's name (the `App`'s) and the content
//! library epoch (`browser.library-epoch`). Live (computed only while
//! observed): `browser.preview-playing` / `preview-position` (the preview
//! player) and `retro.playing` / `position` / `playhead` (the audition
//! mailbox).
//!
//! **Identity.** Sounds are keyed (track instance id, patch id): registered
//! for the palette's track, dropped when the palette leaves it or closes, so
//! a held sound never becomes another patch. Preset files are kept by path,
//! editor macros by (library, name), editor assets by reference, MIDI inputs
//! by device id, slot presets by their rack slot device: positional
//! instances, each allocated an id per key while listed
//! (`registry::KeyedRows`, re-keyed on a reorder, as library grooves). Learn rows and capture lanes and items are
//! positional (a new plan or capture re-pushes their values).

use super::*;
use crate::presented::{presented, Palette, Sidebar};
use sequencer::app::sound_palette::PaletteTarget;

/// The area generations last pushed; `None` forces a push.
#[derive(Default)]
struct Seen {
    sidebar: Option<u64>,
    presets: Option<(u64, u64)>,
    palette: Option<u64>,
    editor: Option<u64>,
    editor_sidebar: Option<u64>,
    learn: Option<u64>,
    retro: Option<u64>,
    export: Option<u64>,
    settings: Option<u64>,
    agent: Option<u64>,
}

/// The views' sync state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct PresentedState {
    seen: Seen,
    /// The singletons the last pushes went to: a new instance (a hot reload)
    /// re-pushes every area.
    singletons: [Option<InstanceId>; SINGLETONS.len()],
    /// The sidebar's track and slot devices as last pushed.
    browser_track: Option<Option<InstanceId>>,
    slot_devices: Vec<Option<InstanceId>>,
    slots: KeyedRows,
    /// The Sounds, then the kits.
    presets: KeyedRows,
    /// The track whose sounds are registered, with the theme tint the
    /// colors were pushed under.
    palette_track: Option<InstanceId>,
    palette_tint: Option<ThemeTint>,
    macros: KeyedRows,
    assets: KeyedRows,
    plan_params: HashMap<u64, InstanceId>,
    epoch_params: HashMap<u64, InstanceId>,
    deltas: HashMap<u64, InstanceId>,
    retro_lanes: HashMap<u64, InstanceId>,
    retro_items: HashMap<u64, InstanceId>,
    midi_devices: KeyedRows,
    project_name: Option<Option<String>>,
    library_epoch: Option<u64>,
    /// Area pushes (an area whose generation moved), for tests.
    pub(crate) pushes: u64,
    /// Instances the stale check asked about (one per collection), for
    /// tests.
    pub(crate) liveness_checks: u64,
}

/// The singletons whose instances key the pushes.
const SINGLETONS: [&str; 9] = [
    BROWSER,
    SOUND_PALETTE,
    EDITOR,
    LEARN,
    RETRO,
    SONG_EXPORT,
    SETTINGS,
    AGENT,
    PROJECT,
];

impl PresentedState {
    /// Push every area at the next tick (a schema change, a hot reload); the
    /// registered instances are kept.
    pub(super) fn invalidate(&mut self) {
        self.seen = Seen::default();
        self.browser_track = None;
        self.slot_devices.clear();
        self.palette_tint = None;
        self.project_name = None;
        self.library_epoch = None;
    }

    /// One instance of each collection this sync registers but the sounds
    /// (children of their track): a hot reload drops a kind's instances
    /// together, so one stands for all.
    fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        let rows = [
            &self.slots,
            &self.presets,
            &self.macros,
            &self.assets,
            &self.midi_devices,
        ];
        let positional = [
            &self.plan_params,
            &self.epoch_params,
            &self.deltas,
            &self.retro_lanes,
            &self.retro_items,
        ];
        (rows.into_iter().filter_map(KeyedRows::representative)).chain(
            positional
                .into_iter()
                .filter_map(|rows| rows.values().next()),
        )
    }
}

impl HostKinds {
    /// The views (see the module docs): each area when its generation moved,
    /// the compared fields every tick. `variant_tint` is the tick's
    /// ([`ModelRevision`]): what the palette's colors go through.
    pub(super) fn sync_presented(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        variant_tint: &ThemeTint,
    ) {
        let singletons = SINGLETONS.map(|kind| pusher.singleton(kind));
        let state = &mut self.presented;
        let mut checks = 0;
        let stale = state.singletons != singletons
            || state.representatives().any(|id| {
                checks += 1;
                !pusher.rt.instance_is_live(*id)
            });
        state.liveness_checks += checks;
        if stale {
            state.invalidate();
            state.singletons = singletons;
        }
        let [browser, palette, editor, learn, retro, export, settings, agent, project] = singletons;
        if let Some(browser) = browser {
            self.sync_browser(pusher, app, browser);
        }
        if let Some(palette) = palette {
            self.sync_palette(pusher, palette, variant_tint);
        }
        let state = &mut self.presented;
        if let Some(editor) = editor {
            state.sync_editor(pusher, editor);
        }
        if let Some(learn) = learn {
            state.sync_learn(pusher, learn);
        }
        if let Some(retro) = retro {
            state.sync_retro(pusher, retro);
        }
        if let Some(export) = export {
            state.sync_export(pusher, export);
        }
        if let (Some(settings), Some(project)) = (settings, project) {
            state.sync_settings(pusher, settings, project);
        }
        if let Some(agent) = agent {
            let generation = presented(|p| p.agent.generation());
            if moved(&mut state.seen.agent, generation, &mut state.pushes) {
                let value = presented(|p| *p.agent.get());
                pusher.push(agent, f::AGENT_GENERATION, number(value as f64));
            }
        }
        if let Some(project) = project {
            let name = app.current_project_name.as_deref();
            if state.project_name.as_ref().map(Option::as_deref) != Some(name) {
                pusher.push(project, f::PROJECT_NAME, text(name.unwrap_or_default()));
                state.project_name = Some(name.map(str::to_string));
            }
        }
    }

    /// The sidebar when its area moved, or its track's or slots' instances
    /// changed; the listings when theirs moved; the library epoch.
    fn sync_browser(&mut self, pusher: &mut Pusher<'_>, app: &app::App, browser: InstanceId) {
        let state = &mut self.presented;
        let epoch = crate::content_library_epoch();
        if state.library_epoch != Some(epoch) {
            pusher.push(browser, f::BROWSER_LIBRARY_EPOCH, number(epoch as f64));
            state.library_epoch = Some(epoch);
        }
        let generation = presented(|p| p.sidebar.generation());
        let instances_moved = state.seen.sidebar.is_some()
            && presented(|p| {
                let sidebar = p.sidebar.get();
                let track = self.track_ids.get(sidebar.track).copied().flatten();
                state.browser_track != Some(track)
                    || state.slot_devices.len() != sidebar.slots.len()
                    || (sidebar.slots.iter().zip(&state.slot_devices)).any(|(slot, pushed)| {
                        slot_device(pusher, app, &self.track_ids, slot) != *pushed
                    })
            });
        if instances_moved {
            state.seen.sidebar = None;
        }
        if moved(&mut state.seen.sidebar, generation, &mut state.pushes) {
            let sidebar = presented(|p| p.sidebar.get().clone());
            push_sidebar(pusher, app, state, &self.track_ids, browser, &sidebar);
        }
        let generations = presented(|p| (p.sound_presets.generation(), p.kit_presets.generation()));
        if moved(&mut state.seen.presets, generations, &mut state.pushes) {
            let (sounds, kits) =
                presented(|p| (p.sound_presets.get().clone(), p.kit_presets.get().clone()));
            let keys: Vec<String> = (sounds.iter().chain(&kits))
                .map(|file| format!("{}:{}", file.file_type, file.path))
                .collect();
            let ids = state.presets.reconcile(pusher, PRESET_FILE, &keys);
            let files = sounds.iter().enumerate().chain(kits.iter().enumerate());
            for ((index, file), id) in files.zip(&ids) {
                let Some(id) = *id else { continue };
                pusher.push(id, f::PRESET_FILE_INDEX, number(index as f64));
                pusher.push(id, f::PRESET_FILE_TYPE, text(file.file_type));
                pusher.push(id, f::PRESET_FILE_ICON, text(file.icon));
                pusher.push(id, f::PRESET_FILE_NAME, text(&file.name));
                pusher.push(id, f::PRESET_FILE_PATH, text(&file.path));
                pusher.push(id, f::PRESET_FILE_PADS, number(file.pads as f64));
                pusher.push(id, f::PRESET_FILE_AUTHOR, text(&file.author));
                pusher.push(id, f::PRESET_FILE_TAGS, strings(&file.tags));
            }
            let (sound_ids, kit_ids) = ids.split_at(sounds.len());
            pusher.push(
                browser,
                f::BROWSER_SOUND_PRESETS,
                listed_instances(sound_ids),
            );
            pusher.push(browser, f::BROWSER_KIT_PRESETS, listed_instances(kit_ids));
        }
    }

    /// The palette when its area, its track's instance or the theme tint
    /// moved: its sounds registered for its track (the previous track's
    /// dropped), their fields and the palette's.
    fn sync_palette(&mut self, pusher: &mut Pusher<'_>, palette: InstanceId, tint: &ThemeTint) {
        let state = &mut self.presented;
        let generation = presented(|p| p.palette.generation());
        let track = presented(|p| p.palette.get().as_ref().map(|palette| palette.track));
        let track_id = track.and_then(|track| self.track_ids.get(track).copied().flatten());
        if state.palette_tint.as_ref() != Some(tint) || state.palette_track != track_id {
            state.seen.palette = None;
        }
        if !moved(&mut state.seen.palette, generation, &mut state.pushes) {
            return;
        }
        state.palette_tint = Some(*tint);
        if let Some(previous) = state
            .palette_track
            .filter(|previous| Some(*previous) != track_id)
        {
            if pusher.rt.instance_is_live(previous) {
                pusher.reconcile_children(previous, SOUND, &[]);
            }
        }
        state.palette_track = track_id;
        let open = presented(|p| p.palette.get().clone());
        let (Some(open), Some(track)) = (open, track_id) else {
            pusher.push(palette, f::PALETTE_OPEN, Value::Bool(false));
            pusher.push(palette, f::PALETTE_TRACK, Value::Nil);
            pusher.push(palette, f::PALETTE_TARGET, text(""));
            pusher.push(palette, f::PALETTE_TARGET_ID, number(-1.0));
            pusher.push(palette, f::PALETTE_INSTRUMENT, text(""));
            pusher.push(palette, f::PALETTE_SOUNDS, Value::List(Vec::new()));
            return;
        };
        let sounds = push_sounds(pusher, &open, track);
        let (target, target_id) = match open.target {
            PaletteTarget::Take(id) => ("take", id.0 as f64),
            PaletteTarget::Pattern(id) => ("pattern", id.0 as f64),
            PaletteTarget::Cell => ("cell", -1.0),
        };
        pusher.push(palette, f::PALETTE_OPEN, Value::Bool(true));
        pusher.push(palette, f::PALETTE_TRACK, Value::Instance(track));
        pusher.push(palette, f::PALETTE_TARGET, text(target));
        pusher.push(palette, f::PALETTE_TARGET_ID, number(target_id));
        pusher.push(palette, f::PALETTE_INSTRUMENT, text(&open.instrument));
        pusher.push(palette, f::PALETTE_SOUNDS, instance_list(sounds));
    }
}

/// The rack slot device `slot` presets name (its track's slot device), when
/// the device sync has it.
fn slot_device(
    pusher: &Pusher<'_>,
    app: &app::App,
    tracks: &[Option<InstanceId>],
    slot: &crate::presented::SlotPresets,
) -> Option<InstanceId> {
    let track = tracks.get(slot.track).copied().flatten()?;
    let did = DeviceSlot::RackSlot(slot.slot).did(app, slot.track);
    pusher.rt.keyed_instance(DEVICE, &[track, did])
}

/// The sidebar's fields and its slots.
fn push_sidebar(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    state: &mut PresentedState,
    tracks: &[Option<InstanceId>],
    browser: InstanceId,
    sidebar: &Sidebar,
) {
    let track = tracks.get(sidebar.track).copied().flatten();
    state.browser_track = Some(track);
    pusher.push(browser, f::BROWSER_TRACK, instance_or_nil(track));
    pusher.push(
        browser,
        f::BROWSER_INSTRUMENT_KIND,
        text(sidebar.instrument_kind),
    );
    pusher.push(browser, f::BROWSER_INSTRUMENT, text(&sidebar.instrument));
    pusher.push(
        browser,
        f::BROWSER_INSTRUMENT_LABEL,
        text(&sidebar.instrument_label),
    );
    pusher.push(browser, f::BROWSER_PRESET, text(&sidebar.preset));
    pusher.push(browser, f::BROWSER_PRESETS, strings(&sidebar.presets));
    pusher.push(
        browser,
        f::BROWSER_USER_PRESETS,
        strings(&sidebar.user_presets),
    );
    pusher.push(browser, f::BROWSER_SAMPLE, text(&sidebar.sample));
    pusher.push(browser, f::BROWSER_ENGINES, strings(&sidebar.engines));
    state.slot_devices = (sidebar.slots.iter())
        .map(|slot| slot_device(pusher, app, tracks, slot))
        .collect();
    let keys: Vec<String> = (sidebar.slots.iter().zip(&state.slot_devices))
        .map(|(slot, device)| match device {
            Some(device) => format!("device:{device}"),
            None => format!("slot:{}:{}", slot.track, slot.slot),
        })
        .collect();
    let ids = state.slots.reconcile(pusher, SLOT_PRESETS, &keys);
    for ((slot, device), id) in sidebar.slots.iter().zip(&state.slot_devices).zip(&ids) {
        let Some(id) = *id else { continue };
        pusher.push(id, f::SLOT_PRESETS_INDEX, number(slot.slot as f64));
        pusher.push(id, f::SLOT_PRESETS_DEVICE, instance_or_nil(*device));
        pusher.push(id, f::SLOT_PRESETS_INSTRUMENT, text(&slot.instrument));
        pusher.push(
            id,
            f::SLOT_PRESETS_INSTRUMENT_LABEL,
            text(&slot.instrument_label),
        );
        pusher.push(id, f::SLOT_PRESETS_PRESETS, strings(&slot.presets));
        pusher.push(
            id,
            f::SLOT_PRESETS_USER_PRESETS,
            strings(&slot.user_presets),
        );
        pusher.push(id, f::SLOT_PRESETS_PRESET, text(&slot.preset));
    }
    pusher.push(browser, f::BROWSER_SLOTS, listed_instances(&ids));
}

/// The open palette's sounds, registered under `track` by patch id (the
/// others of the track dropped), with their fields.
fn push_sounds(pusher: &mut Pusher<'_>, palette: &Palette, track: InstanceId) -> Vec<InstanceId> {
    let wanted: Vec<u64> = palette.entries.iter().map(|entry| entry.patch.0).collect();
    let (ids, changed) =
        reconcile_children(&mut *pusher.rt, track, SOUND, &wanted, |rt, id, patch| {
            rt.push(id, f::SOUND_TRACK, Value::Instance(track));
            rt.push(id, f::SOUND_PATCH_ID, number(patch as f64));
        });
    pusher.changed |= changed;
    let gray = eseqlisp::widget_render::timeline::SOUND_DOT_GRAY;
    for (entry, id) in palette.entries.iter().zip(&ids) {
        let Some(id) = *id else { continue };
        let color = sound_palette_rgb(entry.color);
        let rgb = color.map_or([gray.r, gray.g, gray.b], |(_, rgb)| rgb);
        let mix = entry.mix.map_or(-1.0, |mix| mix.0 as f64);
        pusher.push(id, f::SOUND_MIX_ID, number(mix));
        pusher.push(id, f::SOUND_NAME, text(&entry.name));
        pusher.push(id, f::SOUND_REFERENTS, text(&entry.referents));
        pusher.push(id, f::SOUND_REFERENTS_SHORT, text(&entry.referents_short));
        pusher.push(id, f::SOUND_BASE, Value::Bool(entry.is_base));
        pusher.push(id, f::SOUND_TRACK_SOUND, Value::Bool(entry.is_track_sound));
        pusher.push(id, f::SOUND_CURRENT, Value::Bool(entry.is_current));
        pusher.push(
            id,
            f::SOUND_PRESET,
            text(entry.preset.as_deref().unwrap_or("")),
        );
        pusher.push(
            id,
            f::SOUND_SAMPLE,
            text(entry.sample.as_deref().unwrap_or("")),
        );
        pusher.push(id, f::SOUND_DIFF_UP, number(entry.params_up as f64));
        pusher.push(id, f::SOUND_DIFF_DOWN, number(entry.params_down as f64));
        pusher.push(id, f::SOUND_COLORED, Value::Bool(color.is_some()));
        pusher.push(id, f::SOUND_COLOR, rgb3(rgb));
        let glyph = sound_glyph_key(palette.track, entry.patch.0);
        pusher.push(id, f::SOUND_GLYPH_KEY, Value::String(glyph));
    }
    ids.into_iter().flatten().collect()
}

/// Positional rows of `kind`, one per index below `count` (the instances
/// past it dropped).
fn positional(
    pusher: &mut Pusher<'_>,
    kind: &str,
    known: &mut HashMap<u64, InstanceId>,
    count: usize,
) -> Vec<Option<InstanceId>> {
    let model: Vec<u64> = (0..count as u64).collect();
    reconcile(pusher, kind, known, &model)
}

impl PresentedState {
    /// The editor's fields, and its macro sidebar's, each when its area
    /// moved.
    fn sync_editor(&mut self, pusher: &mut Pusher<'_>, editor: InstanceId) {
        let generation = presented(|p| p.editor.generation());
        if moved(&mut self.seen.editor, generation, &mut self.pushes) {
            let view = presented(|p| p.editor.get().clone());
            pusher.push(editor, f::EDITOR_MODE, text(&view.mode));
            pusher.push(editor, f::EDITOR_SURFACE, text(&view.surface));
            pusher.push(editor, f::EDITOR_BUFFER, text(&view.buffer));
            pusher.push(editor, f::EDITOR_ERROR, text(&view.error));
            pusher.push(editor, f::EDITOR_CANCELING, Value::Bool(view.canceling));
            pusher.push(editor, f::EDITOR_RUN_MODE, text(&view.run_mode));
            pusher.push(editor, f::EDITOR_ACTIVE_MACRO, text(&view.active_macro));
            pusher.push(
                editor,
                f::EDITOR_ACTIVE_MACRO_ACTION,
                text(&view.active_macro_action),
            );
            pusher.push(editor, f::EDITOR_OPEN_MACRO, text(&view.open_macro));
        }
        let generation = presented(|p| p.editor_sidebar.generation());
        if !moved(&mut self.seen.editor_sidebar, generation, &mut self.pushes) {
            return;
        }
        let sidebar = presented(|p| p.editor_sidebar.get().clone());
        let macros = || {
            let patch = sidebar.patch_macros.iter().map(|m| (false, m));
            patch.chain(sidebar.library_macros.iter().map(|m| (true, m)))
        };
        let keys: Vec<String> = macros()
            .map(|(library, m)| format!("{}:{}", if library { "library" } else { "patch" }, m.name))
            .collect();
        let ids = self.macros.reconcile(pusher, EDITOR_MACRO, &keys);
        for ((library, m), id) in macros().zip(&ids) {
            let Some(id) = *id else { continue };
            pusher.push(id, f::EDITOR_MACRO_NAME, text(&m.name));
            pusher.push(id, f::EDITOR_MACRO_LIBRARY, Value::Bool(library));
            pusher.push(id, f::EDITOR_MACRO_PARAMS, strings(&m.params));
            pusher.push(id, f::EDITOR_MACRO_CALLS, strings(&m.calls));
            pusher.push(id, f::EDITOR_MACRO_OUTPUTS, strings(&m.outputs));
            pusher.push(id, f::EDITOR_MACRO_SUMMARY, text(&m.summary));
            pusher.push(id, f::EDITOR_MACRO_USED, Value::Bool(m.used));
        }
        let (patch, library) = ids.split_at(sidebar.patch_macros.len());
        pusher.push(editor, f::EDITOR_PATCH_MACROS, listed_instances(patch));
        pusher.push(editor, f::EDITOR_LIBRARY_MACROS, listed_instances(library));
        let keys: Vec<&str> = (sidebar.assets.iter())
            .map(|asset| asset.reference.as_str())
            .collect();
        let ids = self.assets.reconcile(pusher, EDITOR_ASSET, &keys);
        for (index, (asset, id)) in sidebar.assets.iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::EDITOR_ASSET_INDEX, number(index as f64));
            pusher.push(id, f::EDITOR_ASSET_REFERENCE, text(&asset.reference));
            pusher.push(id, f::EDITOR_ASSET_TIER, text(&asset.tier));
            pusher.push(id, f::EDITOR_ASSET_SOURCE_PATH, text(&asset.source_path));
        }
        pusher.push(editor, f::EDITOR_ASSETS, listed_instances(&ids));
        let selected = match (&sidebar.selected_asset, pusher.singleton(ASSET_INFO)) {
            (Some(asset), Some(info)) => {
                // No metadata (an unresolvable reference): the reference
                // alone, every count 0.
                let meta = asset.metadata.clone().unwrap_or_default();
                let shape: Vec<f64> = meta.shape.iter().map(|d| *d as f64).collect();
                let label = |label: &Option<String>| text(label.as_deref().unwrap_or_default());
                let labels = |labels: &Option<Vec<String>>| strings(labels.iter().flatten());
                pusher.push(info, f::ASSET_REFERENCE, text(&asset.reference));
                pusher.push(info, f::ASSET_TENSOR_KIND, label(&meta.kind));
                pusher.push(info, f::ASSET_LAYOUT, label(&meta.layout));
                pusher.push(info, f::ASSET_SHAPE, numbers(&shape));
                pusher.push(info, f::ASSET_SOURCE, label(&meta.source));
                pusher.push(info, f::ASSET_WAVE_COUNT, number(meta.wave_count as f64));
                let per_set = meta.waves_per_set.unwrap_or(0);
                pusher.push(info, f::ASSET_WAVES_PER_SET, number(per_set as f64));
                pusher.push(info, f::ASSET_SET_COUNT, number(meta.set_count as f64));
                pusher.push(info, f::ASSET_SETS, labels(&meta.sets));
                pusher.push(info, f::ASSET_WAVE_NAMES, labels(&meta.wave_names));
                Value::Instance(info)
            }
            _ => Value::Nil,
        };
        pusher.push(editor, f::EDITOR_SELECTED_ASSET, selected);
    }

    /// Patch Learn when its area moved.
    fn sync_learn(&mut self, pusher: &mut Pusher<'_>, learn: InstanceId) {
        let generation = presented(|p| p.learn.generation());
        if !moved(&mut self.seen.learn, generation, &mut self.pushes) {
            return;
        }
        let view = presented(|p| p.learn.get().clone());
        let texts = [
            (f::LEARN_TARGET_PATH, &view.target_path),
            (f::LEARN_TARGET_NAME, &view.target_name),
            (f::LEARN_PHASE, &view.phase),
            (f::LEARN_METHOD, &view.method),
            (f::LEARN_CMA_REFINE_MODE, &view.cma_refine_mode),
            (f::LEARN_STAGE, &view.stage),
            (f::LEARN_BASIN_CHECK, &view.basin_check),
            (f::LEARN_SEEDED_WAV, &view.seeded_wav),
            (f::LEARN_FINAL_WAV, &view.final_wav),
            (f::LEARN_ERROR, &view.error),
        ];
        for (key, value) in texts {
            pusher.push(learn, key, text(value));
        }
        let numbers_ = [
            (f::LEARN_EPOCHS, view.epochs),
            (f::LEARN_CMA_GENERATIONS, view.cma_generations),
            (f::LEARN_CMA_POPULATION, view.cma_population),
            (f::LEARN_CMA_SIGMA, view.cma_sigma),
            (f::LEARN_CMA_SEED, view.cma_seed),
            (f::LEARN_CMA_FORWARD_BATCH, view.cma_forward_batch),
            (f::LEARN_LOCAL_EPOCHS, view.local_epochs),
            (f::LEARN_CMA_CONTINUE, view.cma_continue),
            (f::LEARN_CMA_REFINE_EPOCHS, view.cma_refine_epochs),
            (f::LEARN_CMA_FINAL_EPOCHS, view.cma_final_epochs),
            (f::LEARN_PITCH_HZ, view.pitch_hz),
            (f::LEARN_GATE_FRAMES, view.gate_frames),
            (f::LEARN_CURRENT_EPOCH, view.current_epoch),
            (f::LEARN_TOTAL_EPOCHS, view.total_epochs),
            (f::LEARN_LOSS, view.loss),
            (f::LEARN_IMPROVEMENT_PCT, view.improvement_pct),
            (f::LEARN_ABS_DISTANCE, view.abs_distance),
        ];
        for (key, value) in numbers_ {
            pusher.push(learn, key, number(value));
        }
        pusher.push(learn, f::LEARN_APPLIED, Value::Bool(view.applied));
        pusher.push(learn, f::LEARN_LOSSES, numbers(&view.losses));
        pusher.push(
            learn,
            f::LEARN_OPTIMIZATION_LOSSES,
            numbers(&view.optimization_losses),
        );
        let rows = positional(
            pusher,
            LEARN_PLAN_PARAM,
            &mut self.plan_params,
            view.plan_params.len(),
        );
        for (index, (row, id)) in view.plan_params.iter().zip(&rows).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::PLAN_PARAM_INDEX, number(index as f64));
            pusher.push(id, f::PLAN_PARAM_NAME, text(&row.name));
            pusher.push(id, f::PLAN_PARAM_STATUS, text(&row.status));
            pusher.push(id, f::PLAN_PARAM_REASON, text(&row.reason));
        }
        pusher.push(learn, f::LEARN_PLAN_PARAMS, listed_instances(&rows));
        let rows = positional(
            pusher,
            LEARN_EPOCH_PARAM,
            &mut self.epoch_params,
            view.epoch_params.len(),
        );
        for (index, (row, id)) in view.epoch_params.iter().zip(&rows).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::EPOCH_PARAM_INDEX, number(index as f64));
            pusher.push(id, f::EPOCH_PARAM_NAME, text(&row.name));
            pusher.push(id, f::EPOCH_PARAM_FROM, number(row.from));
            pusher.push(id, f::EPOCH_PARAM_VALUE, number(row.value));
            pusher.push(id, f::EPOCH_PARAM_CHANGE, number(row.change));
            pusher.push(id, f::EPOCH_PARAM_STEP, number(row.step));
        }
        pusher.push(learn, f::LEARN_EPOCH_PARAMS, listed_instances(&rows));
        let rows = positional(
            pusher,
            LEARN_DELTA,
            &mut self.deltas,
            view.result_deltas.len(),
        );
        for (index, (row, id)) in view.result_deltas.iter().zip(&rows).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::DELTA_INDEX, number(index as f64));
            pusher.push(id, f::DELTA_NAME, text(&row.name));
            pusher.push(id, f::DELTA_FROM, number(row.from));
            pusher.push(id, f::DELTA_TO, number(row.to));
            pusher.push(id, f::DELTA_CHANGE, number(row.change));
        }
        pusher.push(learn, f::LEARN_RESULT_DELTAS, listed_instances(&rows));
    }

    /// The MIDI capture when its area moved.
    fn sync_retro(&mut self, pusher: &mut Pusher<'_>, retro: InstanceId) {
        let generation = presented(|p| p.retro.generation());
        if !moved(&mut self.seen.retro, generation, &mut self.pushes) {
            return;
        }
        let view = presented(|p| p.retro.get().clone());
        let lanes = positional(pusher, RETRO_LANE, &mut self.retro_lanes, view.lanes.len());
        for (index, (label, id)) in view.lanes.iter().zip(&lanes).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::RETRO_LANE_INDEX, number(index as f64));
            pusher.push(id, f::RETRO_LANE_LABEL, text(label));
        }
        let items = positional(pusher, RETRO_ITEM, &mut self.retro_items, view.items.len());
        for (index, (item, id)) in view.items.iter().zip(&items).enumerate() {
            let Some(id) = *id else { continue };
            let lane = lanes.get(item.lane).copied().flatten();
            pusher.push(id, f::RETRO_ITEM_INDEX, number(index as f64));
            pusher.push(id, f::RETRO_ITEM_LANE, instance_or_nil(lane));
            pusher.push(id, f::RETRO_ITEM_START, number(item.start));
            pusher.push(id, f::RETRO_ITEM_END, number(item.end));
        }
        pusher.push(retro, f::RETRO_LANES, listed_instances(&lanes));
        pusher.push(retro, f::RETRO_ITEMS, listed_instances(&items));
        pusher.push(retro, f::RETRO_DURATION, number(view.duration));
        pusher.push(retro, f::RETRO_TRUNCATED, Value::Bool(view.truncated));
        pusher.push(retro, f::RETRO_ERROR, text(&view.error));
    }

    /// The song export when its area moved.
    fn sync_export(&mut self, pusher: &mut Pusher<'_>, export: InstanceId) {
        let generation = presented(|p| p.export.generation());
        if !moved(&mut self.seen.export, generation, &mut self.pushes) {
            return;
        }
        let view = presented(|p| p.export.get().clone());
        pusher.push(export, f::EXPORT_DEFAULT_NAME, text(&view.default_name));
        pusher.push(export, f::EXPORT_PROJECT, text(&view.project));
        pusher.push(export, f::EXPORT_FOLDER, text(&view.folder));
        pusher.push(export, f::EXPORT_END, number(view.end));
        pusher.push(export, f::EXPORT_BUSY, Value::Bool(view.busy));
        pusher.push(export, f::EXPORT_DONE, Value::Bool(view.done));
        pusher.push(export, f::EXPORT_MESSAGE, text(&view.message));
        pusher.push(export, f::EXPORT_PERCENT, number(view.percent));
        pusher.push(export, f::EXPORT_OUTPUT_NAME, text(&view.output_name));
        pusher.push(export, f::EXPORT_REVEAL_LABEL, text(&view.reveal_label));
    }

    /// The settings (and the project's audio worker options) when their area
    /// moved.
    fn sync_settings(
        &mut self,
        pusher: &mut Pusher<'_>,
        settings: InstanceId,
        project: InstanceId,
    ) {
        let generation = presented(|p| p.settings.generation());
        if !moved(&mut self.seen.settings, generation, &mut self.pushes) {
            return;
        }
        let view = presented(|p| p.settings.get().clone());
        pusher.push(
            settings,
            f::SETTINGS_AUDIO_WORKERS_CHOICE,
            text(&view.workers_choice),
        );
        pusher.push(
            settings,
            f::SETTINGS_AUDIO_WORKERS_NOTE,
            text(&view.workers_note),
        );
        pusher.push(
            project,
            f::PROJECT_AUDIO_WORKERS_OPTIONS,
            strings(&view.workers_options),
        );
        let keys: Vec<&str> = view.midi_devices.iter().map(|d| d.id.as_str()).collect();
        let ids = self.midi_devices.reconcile(pusher, MIDI_DEVICE, &keys);
        for (index, (device, id)) in view.midi_devices.iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::MIDI_DEVICE_INDEX, number(index as f64));
            pusher.push(id, f::MIDI_DEVICE_ID, text(&device.id));
            pusher.push(id, f::MIDI_DEVICE_NAME, text(&device.name));
            pusher.push(id, f::MIDI_DEVICE_ENABLED, Value::Bool(device.enabled));
            pusher.push(id, f::MIDI_DEVICE_CONNECTED, Value::Bool(device.connected));
            pusher.push(id, f::MIDI_DEVICE_STATUS, text(&device.status));
        }
        pusher.push(settings, f::SETTINGS_MIDI_DEVICES, listed_instances(&ids));
        pusher.push(settings, f::SETTINGS_MIDI_ERROR, text(&view.midi_error));
        pusher.push(
            settings,
            f::SETTINGS_MIDI_PERSISTENT,
            Value::Bool(view.midi_persistent),
        );
    }
}
