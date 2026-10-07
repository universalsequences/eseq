//! The legacy reactive names the record mirrors (spec §13 stage 8,
//! §14.2i): `SEQ.editor-active` / `editor-mode`, `EXPORT.export-*` and
//! `AGENT.generation`. (`RETRO.*` went with the MIDI capture view's port,
//! eseq-0l17.12: the capture area is unmirrored; the preset listings' rows
//! and most editor fields with the browser's, eseq-0l17.17; Patch Learn,
//! the editor's macro sidebar, `AUDIO` and `MIDI.devices` / `error` /
//! `persistent` with the patching and settings views', eseq-0l17.18.)
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

/// The editor fields unported views still read (the browser reads the
/// `editor` host kind).
fn editor_fields(old: Option<&EditorView>, new: &EditorView, emit: Emit<'_>) {
    fields!(old, new, emit;
        "editor-active" => active: flag,
        "editor-mode" => mode: text,
    );
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

pub(super) fn mirror_export(sink: &mut dyn Sink, old: &ExportView, new: &ExportView) {
    mirror(sink, "EXPORT", old, new, export_fields);
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

/// The editor's `SEQ` fields, for its registration.
pub(crate) fn seq_registration() -> Vec<(&'static str, Value)> {
    registration(|p| p.editor.get(), editor_fields)
}

/// An area no legacy name mirrors any more (its views read the kinds).
pub(super) fn unmirrored<T>(_sink: &mut dyn Sink, _old: &T, _new: &T) {}

/// The `EXPORT` fields.
pub(crate) fn export_registration() -> Vec<(&'static str, Value)> {
    registration(|p| p.export.get(), export_fields)
}

/// The `AGENT` fields.
pub(crate) fn agent_registration() -> Vec<(&'static str, Value)> {
    vec![(
        "generation",
        Value::Number(presented(|p| *p.agent.get()) as f64),
    )]
}
