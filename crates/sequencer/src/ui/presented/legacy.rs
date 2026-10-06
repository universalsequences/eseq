//! The legacy reactive names the record mirrors (spec §13 stage 8,
//! §14.2i): `SEQ.editor-*`, `SEQ.learn-*`, `EXPORT.export-*`, `AUDIO.*`,
//! `MIDI.devices` / `error` / `persistent`, `AGENT.generation`, and the
//! preset listings' rows. (`RETRO.*` went with the MIDI capture view's port,
//! eseq-0l17.12: the capture area is unmirrored.)
//!
//! Every legacy name lives here. Each area's fields are listed once
//! ([`fields!`]): a typed edit writes the ones that changed
//! (`mirror_*`), a registration all of them as the record holds them now
//! (`*_registration`), so the two never disagree. The values are derived
//! from the record; nothing is parsed back. eseq-0l17.22 deletes this file
//! and the mirror calls in `presented`.

use super::*;

/// A legacy field write: its name and value.
type Emit<'a> = &'a mut dyn FnMut(&'static str, Value);

/// Emit each listed legacy field of `$new` whose source field differs from
/// `$old`'s (every field when `$old` is `None`), converted by its function.
macro_rules! fields {
    ($old:expr, $new:expr, $emit:expr; $($name:literal => $field:ident: $value:expr,)*) => {{
        let (old, new, emit) = ($old, $new, $emit);
        $(
            if old.is_none_or(|old| old.$field != new.$field) {
                emit($name, $value(&new.$field));
            }
        )*
    }};
}

fn text(text: &str) -> Value {
    Value::String(text.to_string())
}

fn flag(on: &bool) -> Value {
    Value::Bool(*on)
}

fn number(n: &f64) -> Value {
    Value::Number(*n)
}

fn numbers(values: &[f64]) -> Value {
    list_value(values.iter().map(number))
}

fn strings(values: &[String]) -> Value {
    list_value(values.iter().map(|value| text(value)))
}

fn editor_fields(old: Option<&EditorView>, new: &EditorView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "editor-active" => active: flag,
        "editor-mode" => mode: text,
        "editor-surface" => surface: text,
        "editor-buffer-name" => buffer: text,
        "editor-error" => error: text,
        "editor-canceling" => canceling: flag,
        "editor-instrument-run-mode" => run_mode: text,
        "editor-active-macro-name" => active_macro: text,
        "editor-active-macro-action" => active_macro_action: text,
        "editor-open-macro" => open_macro: text,
    );
}

fn editor_sidebar_fields(old: Option<&EditorSidebar>, new: &EditorSidebar, emit: Emit<'_>) {
    fields!(old, new, emit;
        "editor-patch-macros" => patch_macros: patch_macro_rows,
        "editor-library-macros" => library_macros: library_macro_rows,
        "editor-assets" => assets: asset_rows,
        "editor-selected-asset" => selected_asset: selected_asset,
    );
}

fn patch_macro_rows(macros: &[EditorMacro]) -> Value {
    list_value(macros.iter().map(|m| {
        map_value([
            ("name", text(&m.name)),
            ("params", strings(&m.params)),
            ("calls", strings(&m.calls)),
        ])
    }))
}

fn library_macro_rows(macros: &[EditorMacro]) -> Value {
    list_value(macros.iter().map(|m| {
        map_value([
            ("name", text(&m.name)),
            ("params", strings(&m.params)),
            ("outputs", strings(&m.outputs)),
            ("summary", text(&m.summary)),
            ("calls", strings(&m.calls)),
            ("used", flag(&m.used)),
        ])
    }))
}

fn asset_rows(assets: &[EditorAsset]) -> Value {
    list_value(assets.iter().map(|asset| {
        map_value([
            ("label", text(&asset.reference)),
            ("name", text(&asset.reference)),
            ("kind", Value::String("patcher-asset".to_string())),
            ("detail", text(&asset.tier)),
            ("tier", text(&asset.tier)),
            ("file", text(&asset.reference)),
            ("source-path", text(&asset.source_path)),
            ("drag-type", Value::String("dgen-asset".to_string())),
            ("draggable", Value::Bool(true)),
            ("drop-target", Value::Bool(false)),
        ])
    }))
}

/// The asset's metadata map (empty when it resolves to none) with its
/// reference; nil when no asset is selected.
fn selected_asset(asset: &Option<AssetInfo>) -> Value {
    let Some(asset) = asset else {
        return Value::Nil;
    };
    let mut fields = match asset.metadata.as_ref().map(|metadata| metadata.value()) {
        Some(Value::Map(fields)) => fields,
        _ => HashMap::new(),
    };
    fields.insert(
        "reference".to_string(),
        Rc::new(RefCell::new(text(&asset.reference))),
    );
    Value::Map(fields)
}

fn learn_fields(old: Option<&LearnView>, new: &LearnView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "learn-target-path" => target_path: text,
        "learn-target-name" => target_name: text,
        "learn-phase" => phase: text,
        "learn-plan-params" => plan_params: plan_rows,
        "learn-method" => method: text,
        "learn-epochs" => epochs: number,
        "learn-cma-generations" => cma_generations: number,
        "learn-cma-population" => cma_population: number,
        "learn-cma-sigma" => cma_sigma: number,
        "learn-cma-seed" => cma_seed: number,
        "learn-cma-forward-batch" => cma_forward_batch: number,
        "learn-local-epochs" => local_epochs: number,
        "learn-cma-continue" => cma_continue: number,
        "learn-cma-refine-epochs" => cma_refine_epochs: number,
        "learn-cma-refine-mode" => cma_refine_mode: text,
        "learn-cma-final-epochs" => cma_final_epochs: number,
        "learn-pitch-hz" => pitch_hz: number,
        "learn-gate-frames" => gate_frames: number,
        "learn-stage" => stage: text,
        "learn-current-epoch" => current_epoch: number,
        "learn-total-epochs" => total_epochs: number,
        "learn-loss" => loss: number,
        "learn-losses" => losses: numbers,
        "learn-optimization-losses" => optimization_losses: numbers,
        "learn-epoch-params" => epoch_params: epoch_rows,
        "learn-checkpoint-wav" => checkpoint_wav: text,
        "learn-improvement-pct" => improvement_pct: number,
        "learn-abs-distance" => abs_distance: number,
        "learn-basin-check" => basin_check: text,
        "learn-result-deltas" => result_deltas: delta_rows,
        "learn-seeded-wav" => seeded_wav: text,
        "learn-final-wav" => final_wav: text,
        "learn-applied" => applied: flag,
        "learn-error" => error: text,
    );
}

fn plan_rows(rows: &[LearnPlanParam]) -> Value {
    list_value(rows.iter().map(|row| {
        map_value([
            ("name", text(&row.name)),
            ("status", text(&row.status)),
            ("reason", text(&row.reason)),
        ])
    }))
}

fn epoch_rows(rows: &[LearnEpochParam]) -> Value {
    list_value(rows.iter().map(|row| {
        map_value([
            ("name", text(&row.name)),
            ("from", number(&row.from)),
            ("value", number(&row.value)),
            ("change", number(&row.change)),
            ("step", number(&row.step)),
        ])
    }))
}

fn delta_rows(rows: &[LearnDelta]) -> Value {
    list_value(rows.iter().map(|row| {
        map_value([
            ("name", text(&row.name)),
            ("from", number(&row.from)),
            ("to", number(&row.to)),
            ("change", number(&row.change)),
        ])
    }))
}

fn export_fields(old: Option<&ExportView>, new: &ExportView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "export-default-name" => default_name: text,
        "export-project" => project: text,
        "export-folder" => folder: text,
        "export-end" => end: number,
        "export-busy" => busy: flag,
        "export-done" => done: flag,
        "export-message" => message: text,
        "export-percent" => percent: number,
        "export-output-name" => output_name: text,
        "export-reveal-label" => reveal_label: text,
    );
}

fn audio_fields(old: Option<&SettingsView>, new: &SettingsView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "workers-choice" => workers_choice: text,
        "workers-options" => workers_options: strings,
        "workers-note" => workers_note: text,
    );
}

fn midi_fields(old: Option<&SettingsView>, new: &SettingsView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "devices" => midi_devices: midi_devices,
        "error" => midi_error: text,
        "persistent" => midi_persistent: flag,
    );
}

fn midi_devices(devices: &[MidiDevice]) -> Value {
    list_value(devices.iter().map(|device| {
        map_value([
            ("id", text(&device.id)),
            ("name", text(&device.name)),
            ("enabled", flag(&device.enabled)),
            ("connected", flag(&device.connected)),
            ("status", text(&device.status)),
        ])
    }))
}

/// Where the mirror writes: the runtime, or a native's context (a capture
/// fixture's `present-fixture`, applied when the native returns).
pub(crate) trait Sink {
    fn write(&mut self, namespace: &'static str, field: &'static str, value: Value);
}

impl Sink for Runtime {
    fn write(&mut self, namespace: &'static str, field: &'static str, value: Value) {
        self.set_reactive(namespace, field, value);
    }
}

impl Sink for eseqlisp::runtime::NativeContext {
    fn write(&mut self, namespace: &'static str, field: &'static str, value: Value) {
        self.reactive_set(namespace, field, value);
    }
}

/// Write `fields`' changed legacy fields into `namespace`.
fn mirror<T>(
    sink: &mut dyn Sink,
    namespace: &'static str,
    old: &T,
    new: &T,
    fields: fn(Option<&T>, &T, Emit<'_>),
) {
    fields(Some(old), new, &mut |name, value| {
        sink.write(namespace, name, value)
    });
}

pub(super) fn mirror_editor(sink: &mut dyn Sink, old: &EditorView, new: &EditorView) {
    mirror(sink, "SEQ", old, new, editor_fields);
}

pub(super) fn mirror_editor_sidebar(sink: &mut dyn Sink, old: &EditorSidebar, new: &EditorSidebar) {
    mirror(sink, "SEQ", old, new, editor_sidebar_fields);
}

pub(super) fn mirror_learn(sink: &mut dyn Sink, old: &LearnView, new: &LearnView) {
    mirror(sink, "SEQ", old, new, learn_fields);
}

pub(super) fn mirror_export(sink: &mut dyn Sink, old: &ExportView, new: &ExportView) {
    mirror(sink, "EXPORT", old, new, export_fields);
}

pub(super) fn mirror_settings(sink: &mut dyn Sink, old: &SettingsView, new: &SettingsView) {
    mirror(sink, "AUDIO", old, new, audio_fields);
    mirror(sink, "MIDI", old, new, midi_fields);
}

pub(super) fn mirror_agent(sink: &mut dyn Sink, _old: &u64, new: &u64) {
    sink.write("AGENT", "generation", Value::Number(*new as f64));
}

/// Every field `fields` lists, as the record holds it now.
fn registration<T>(
    area: impl FnOnce(&Presented) -> &T,
    fields: fn(Option<&T>, &T, Emit<'_>),
) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    presented(|p| fields(None, area(p), &mut |name, value| out.push((name, value))));
    out
}

/// The editor's and Patch Learn's `SEQ` fields, for its registration.
pub(crate) fn seq_registration() -> Vec<(&'static str, Value)> {
    let mut fields = registration(|p| p.editor.get(), editor_fields);
    fields.extend(registration(
        |p| p.editor_sidebar.get(),
        editor_sidebar_fields,
    ));
    fields.extend(registration(|p| p.learn.get(), learn_fields));
    fields
}

/// An area no legacy name mirrors any more (its views read the kinds).
pub(super) fn unmirrored<T>(_sink: &mut dyn Sink, _old: &T, _new: &T) {}

/// The `EXPORT` fields.
pub(crate) fn export_registration() -> Vec<(&'static str, Value)> {
    registration(|p| p.export.get(), export_fields)
}

/// The `AUDIO` fields and the `MIDI` ones (but the per-port `ports`).
pub(crate) fn settings_registration() -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
    (
        registration(|p| p.settings.get(), audio_fields),
        registration(|p| p.settings.get(), midi_fields),
    )
}

/// The `AGENT` fields.
pub(crate) fn agent_registration() -> Vec<(&'static str, Value)> {
    vec![(
        "generation",
        Value::Number(presented(|p| *p.agent.get()) as f64),
    )]
}

/// The legacy `SEQ.sound-presets` / `SEQ.kit-presets` rows (`:label` and
/// `:name` are both the name; a kit's `:pads`).
pub(crate) fn preset_files_value(files: &[PresetFile]) -> Value {
    list_value(files.iter().map(|file| {
        let mut entries = vec![
            ("kind", Value::String(file.file_type.to_string())),
            ("icon", Value::Keyword(file.icon.to_string())),
            ("label", text(&file.name)),
            ("name", text(&file.name)),
            ("path", text(&file.path)),
        ];
        if file.file_type == "kit" {
            entries.push(("pads", Value::Number(file.pads as f64)));
        }
        entries.push(("author", text(&file.author)));
        entries.push(("tags", strings(&file.tags)));
        map_value(entries)
    }))
}
