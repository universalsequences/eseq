//! The instrument's own eseqlisp panel, booted from the payload with no app
//! around it: the factory UI runtime (`effects.lisp` and the `eseq.kinds`
//! module) renders it exactly as eseq's instrument panel does, from kind
//! instances this module publishes — one track, one instrument device and its
//! params, mirroring the plugin's [`ParamStore`]. Knob edits arrive as the
//! same host commands the app handles and become plugin param edits.
//!
//! Lives on the host's main thread (`Runtime` is not `Send`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use eseqlisp::editor::ViewMode;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::vm::{InstanceId, Value};
use eseqlisp::{Editor, EditorConfig, HostCommand, Runtime};

use eseq_voice_mod as vm;

use crate::engine::{ModDisplay, ParamStore};
use crate::payload::{ParamKind, Payload, PluginSpec};

/// The grid font size eseq's own panels are laid out in.
const FONT_SIZE_PT: f64 = 13.0;
const BUFFER: &str = "*plugin*";
/// How long the panel holds its last frame waiting for a requested resize.
const RESIZE_WAIT: std::time::Duration = std::time::Duration::from_millis(250);
/// Breathing room around the panel, in cells.
const MARGIN_CELLS: f32 = 0.5;

/// A param edit made on the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelEdit {
    Value(usize, f32),
    /// A drag finished (a batch edit's `:commit`, or the pointer released).
    Commit,
}

pub struct PanelHost {
    editor: Editor,
    backend: MetalBackend,
    params: Arc<ParamStore>,
    spec: PluginSpec,
    device: InstanceId,
    param_instances: Vec<InstanceId>,
    shown: Vec<f32>,
    /// The engine's newest-voice modulation (slot values and phases).
    mod_display: Arc<ModDisplay>,
    /// Last published (mod-value, mod-offset, mod-scale) per param.
    mod_shown: Vec<(f32, f32, f32)>,
    phases_shown: [f32; vm::SLOT_COUNT],
    /// A modulation source setting moved: re-decide which settings show.
    sources_dirty: bool,
    cell: (f32, f32),
    scale: f64,
    /// Panel size in points.
    size: (f64, f64),
    /// The panel's size with the mods tab closed, open.
    sizes: [(f64, f64); 2],
    /// When the panel started waiting for the host to apply its new size.
    resize_wait: Option<std::time::Instant>,
    edits: Vec<PanelEdit>,
    /// Input arrived since the last pump: the runtime has side effects and
    /// host commands to settle.
    dirty: bool,
    /// The latest pointer drag, applied once per frame (as the app coalesces).
    pending_drag: Option<(crossterm::event::MouseEvent, f32, f32)>,
    trace: bool,
    unsupported_logged: std::collections::BTreeSet<String>,
}

fn list(values: Vec<Value>) -> Value {
    Value::List(values.into_iter().map(|v| Rc::new(RefCell::new(v))).collect())
}

fn set(rt: &mut Runtime, id: InstanceId, field: &str, value: Value) -> Result<(), String> {
    rt.set_instance_field(id, field, value).map_err(|e| format!("{field}: {e:?}"))
}

fn dict_get(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Map(map) => map.get(key).map(|v| v.borrow().clone()),
        _ => None,
    }
}

fn number(value: Option<Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => Some(n),
        Value::Bool(b) => Some(b as u8 as f64),
        _ => None,
    }
}

impl PanelHost {
    /// Boot the panel of `payload` at `scale` pixels per point. Expensive
    /// (it evaluates the UI runtime): call when the host opens the editor.
    pub fn boot(payload: &Payload, params: Arc<ParamStore>, mod_display: Arc<ModDisplay>, scale: f64) -> Result<Self, String> {
        let spec = payload.spec.clone();
        let ui = spec.ui.clone().ok_or("this plugin has no panel")?;
        let content = payload.dir.join(&ui.root);
        configure_roots(&content);

        let init_path = content.join("core/init.lisp");
        let init_source = std::fs::read_to_string(&init_path).ok();
        let mut runtime = Runtime::new();
        runtime.set_load_root(content.clone());
        runtime.set_scoped_module_load_path(vec![eseqlisp::ModuleLoadRoot {
            path: content.clone(),
            module_prefix: None,
        }]);
        // The factory UI still carries compatibility spellings the app also
        // leaves unmigrated; don't lecture the host's console about them.
        runtime.exclude_module_alias_scan_root(content.join("ui"));
        runtime.exclude_module_alias_scan_root(content.join("core"));
        // The app's natives, as no-ops: panel code that reaches app state
        // (delete targets, step selection, …) reads nil instead of failing
        // the whole callback on an unknown variable. Lisp definitions loaded
        // below still win.
        for name in crate::app_natives::APP_NATIVES {
            runtime.register_native(name, |_args, _ctx| Ok(Value::Nil));
        }
        let mut editor = Editor::new(
            runtime,
            EditorConfig { init_source, init_source_path: Some(init_path), vim_mode: false },
        );

        let mut backend = MetalBackend::new_capture_with_font_size(64, 64, FONT_SIZE_PT)
            .map_err(|_| "Metal device unavailable")?;
        let font = payload.dir.join(&ui.font);
        if font.is_file() {
            backend.set_monospace_font_path(&font).map_err(|_| "monospace font rejected")?;
        }
        backend.initialize_offscreen(scale).map_err(|_| "Metal init failed")?;
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some((text_w, text_h)) = backend.sync_text_zoom(editor.text_zoom()) {
            editor.set_text_cell_dimensions(cell_w, cell_h, text_w, text_h);
        }
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }

        let theme = if ui.theme.is_empty() { "seq-theme-mac-osx-dark".to_string() } else { ui.theme.clone() };
        let panel_source = std::fs::read_to_string(payload.dir.join(&ui.entry))
            .map_err(|e| format!("{}: {e}", ui.entry))?;
        let rt = editor.runtime_mut();
        eval(rt, &format!(
            r#"(load "@/ui/themes.lisp")
               ({theme})
               (import eseq.materials)
               (import eseq.seq-core-state)
               (import eseq.kinds)"#
        ), "runtime")?;
        eval(rt, PLUGIN_STUBS, "stubs")?;
        eval(rt, &panel_source, "panel source")?;
        eval(rt, r#"(load "@/ui/effects.lisp")"#, "effects.lisp")?;
        let kinds = publish_kinds(rt, &spec, &params)?;
        // The header's voices picker binds the voices param instance.
        let voices = spec
            .params
            .iter()
            .position(|p| p.name == crate::payload::VOICES_PARAM)
            .map_or(Value::Nil, |index| Value::Instance(kinds.params[index]));
        rt.register_native("plugin-voices-param", move |_args, _ctx| Ok(voices.clone()));
        eval(rt, PLUGIN_ROOT, "plugin root")?;
        eseqlisp::runtime::recompile_theme_dependent_sdf_shaders();
        editor.refresh_runtime_side_effects();

        let buffer = editor
            .buffers
            .iter()
            .find(|b| b.name == BUFFER)
            .map(|b| b.id)
            .ok_or("the plugin root buffer was not created")?;
        editor.set_active_buffer(buffer);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        // A plugin window is the panel alone: no `*plugin*` mode line.
        editor.active_leaf_mut().show_status = false;
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            crate::shell::log(&format!("panel status: {status}"));
        }

        let cell = backend.cell_dimensions();
        let shown = (0..params.len()).map(|i| params.get(i)).collect();
        let n_params = spec.params.len();
        let mut host = Self {
            editor,
            backend,
            params,
            spec,
            device: kinds.device,
            param_instances: kinds.params,
            shown,
            mod_display,
            mod_shown: vec![(f32::NAN, f32::NAN, f32::NAN); n_params],
            phases_shown: [f32::NAN; vm::SLOT_COUNT],
            sources_dirty: true,
            cell,
            scale,
            size: (0.0, 0.0),
            sizes: [(0.0, 0.0); 2],
            resize_wait: None,
            edits: Vec::new(),
            dirty: false,
            pending_drag: None,
            trace: std::env::var_os("ESEQ_PLUGIN_TRACE").is_some(),
            unsupported_logged: Default::default(),
        };
        host.sizes = host.measure(ui.width, ui.height);
        let (width, height) = host.sizes[0];
        host.resize(width, height);
        Ok(host)
    }

    /// The panel's extent in points: the laid-out panel plus a margin.
    /// The panel's extent in points with the mods tab closed and open
    /// (the window follows the tab), unless the payload fixes a size.
    fn measure(&mut self, width: f64, height: f64) -> [(f64, f64); 2] {
        if width > 0.0 && height > 0.0 {
            return [(width, height); 2];
        }
        // Lay out generously, then shrink to what the panel occupies.
        self.editor.set_layout_viewport(400, 120);
        let mut sizes = [(0.0, 0.0); 2];
        // The mods tab button's own toggle, twice: open, then back closed.
        for open in [true, false] {
            if let Err(e) = self.editor.runtime_mut().eval_str(TOGGLE_MODS) {
                crate::shell::log(&format!("mods toggle: {e:?}"));
            }
            self.editor.refresh_runtime_side_effects();
            let (cols, rows) =
                self.editor.widget_layout().map(|layout| content_extent(&layout)).unwrap_or((60.0, 12.0));
            let (cell_w, cell_h) = self.cell;
            // Whole cells: the renderer lays out on a whole-cell grid, and a
            // fractional row lost to its floor would clip the panel's bottom.
            let pt = |cells: f32, cell: f32| ((cells + 2.0 * MARGIN_CELLS).ceil() * cell) as f64 / self.scale;
            sizes[open as usize] = (pt(cols, cell_w), pt(rows, cell_h));
        }
        sizes
    }

    /// The size the window should have now: the mods tab's when it is open.
    pub fn wanted_size(&mut self) -> (f64, f64) {
        self.sizes[self.mods_open() as usize]
    }

    /// Whether the mods tab is showing (`eseq.effects.state` instrument-view).
    fn mods_open(&self) -> bool {
        let rt = self.editor.runtime();
        rt.singleton_instance(INSTRUMENT_VIEW_KIND)
            .and_then(|view| rt.instance_field(view, "mods-open").ok())
            .is_some_and(|open| matches!(open, Value::Bool(true)))
    }

    pub fn size(&self) -> (f64, f64) {
        self.size
    }

    /// Grid size in cells for the current size.
    fn grid(&self) -> (usize, usize) {
        let (cell_w, cell_h) = self.cell;
        let cols = ((self.size.0 * self.scale) as f32 / cell_w).floor().max(1.0) as usize;
        let rows = ((self.size.1 * self.scale) as f32 / cell_h).floor().max(1.0) as usize;
        (cols, rows)
    }

    /// Attach to the host's view (`NSView*`).
    ///
    /// # Safety
    /// `view` must be a live NSView, used on the main thread.
    pub unsafe fn attach(&mut self, view: *mut std::ffi::c_void) {
        unsafe { self.backend.attach_to_ns_view(view) };
        self.resize(self.size.0, self.size.1);
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        self.size = (width.max(1.0), height.max(1.0));
        self.backend.set_drawable_pixel_size(
            (self.size.0 * self.scale).round() as u32,
            (self.size.1 * self.scale).round() as u32,
        );
        let (cols, rows) = self.grid();
        self.editor.set_layout_viewport(cols as u16, rows as u16);
        self.editor.update_tile_rects(cols as u16, rows as u16);
        self.editor.mark_needs_redraw();
    }

    /// Mirror store values the host or automation changed into the kinds.
    fn sync_from_store(&mut self) {
        let mut changed = false;
        for index in 0..self.shown.len() {
            let value = self.params.get(index);
            if value.to_bits() == self.shown[index].to_bits() {
                continue;
            }
            self.shown[index] = value;
            let id = self.param_instances[index];
            let spec = &self.spec.params[index];
            let text = spec.option_label(value).unwrap_or_default().to_string();
            let display = spec.to_display(value) as f64;
            let rt = self.editor.runtime_mut();
            for field in ["value", "base", "process-value"] {
                let _ = rt.set_instance_field(id, field, Value::Number(display));
            }
            let _ = rt.set_instance_field(id, "text", Value::String(text));
            // The modulated value follows in `sync_modulation`.
            self.mod_shown[index] = (f32::NAN, f32::NAN, f32::NAN);
            self.sources_dirty |= self.spec.params[index].section == "source";
            changed = true;
        }
        changed |= self.sync_source_visibility();
        changed |= self.sync_modulation();
        if changed {
            self.editor.runtime_mut().run_reactive_cycle();
            self.editor.mark_needs_redraw();
            self.dirty = true;
        }
    }

    /// Show each slot's type and the settings its selected type uses (the
    /// app's rule, shared through `eseq-voice-mod`).
    fn sync_source_visibility(&mut self) -> bool {
        if !std::mem::take(&mut self.sources_dirty) {
            return false;
        }
        let sources: Vec<(usize, &str)> = self
            .spec
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.section == "source")
            .map(|(i, p)| (i, p.name.as_str()))
            .collect();
        let visible = vm::selected_source_settings(&sources, |i| self.params.get(i));
        let rt = self.editor.runtime_mut();
        for &(index, _) in &sources {
            let _ = rt.set_instance_field(self.param_instances[index], "visible", Value::Bool(visible.contains(&index)));
        }
        true
    }

    /// The live modulation: each slot's cycle phase (the LFO editor's
    /// playhead) and each modulated param's effective value — base plus
    /// every active lane's depth × slot output (×2^ for octave lanes), in
    /// display units, as the app's `mod-value`/`mod-offset`/`mod-scale`.
    fn sync_modulation(&mut self) -> bool {
        let mut changed = false;
        let phases: [f32; vm::SLOT_COUNT] = std::array::from_fn(|slot| {
            let phase = self.mod_display.phase(slot);
            if phase < 0.0 { -1.0 } else { (phase * 128.0).round() / 128.0 }
        });
        if phases != self.phases_shown {
            self.phases_shown = phases;
            let rt = self.editor.runtime_mut();
            let list_value = list(phases.iter().map(|&p| Value::Number(p as f64)).collect());
            let _ = rt.set_instance_field(self.device, "mod-phases", list_value);
            for (index, p) in self.spec.params.iter().enumerate() {
                if p.section == "source" && p.mod_slot >= 1 {
                    let _ = rt.set_instance_field(
                        self.param_instances[index],
                        "mod-phase",
                        Value::Number(phases[p.mod_slot - 1] as f64),
                    );
                }
            }
            changed = true;
        }
        let slot_values: [f32; vm::SLOT_COUNT] = std::array::from_fn(|slot| self.mod_display.value(slot).clamp(0.0, 1.0));
        let n = self.spec.params.len();
        for base in 0..n {
            let lanes = self.spec.dsp.layout.mod_targets.iter().filter(|t| t.base == base);
            let mut amount = 0.0f32;
            let mut octaves = false;
            let mut any = false;
            for lane in lanes {
                any = true;
                let active = lane.active.is_none_or(|a| self.params.get(a) > 0.5);
                let depth = self.spec.params[lane.depth].to_display(self.params.get(lane.depth));
                if active && depth != 0.0 && (1..=vm::SLOT_COUNT).contains(&lane.slot) {
                    amount += depth * slot_values[lane.slot - 1];
                    octaves |= lane.octaves;
                }
            }
            if !any {
                continue;
            }
            let spec = &self.spec.params[base];
            let display_base = spec.to_display(self.params.get(base));
            let (lo, hi) = (spec.to_display(spec.min), spec.to_display(spec.max));
            let value = if octaves { display_base * amount.exp2() } else { display_base + amount };
            let value = value.clamp(lo.min(hi), hi.max(lo));
            // Two decimals of display precision: no repaint for sub-visible motion.
            let value = (value * 100.0).round() / 100.0;
            let scale = if octaves && display_base != 0.0 { value / display_base } else { 1.0 };
            let shown = (value, value - display_base, scale);
            if shown == self.mod_shown[base] {
                continue;
            }
            self.mod_shown[base] = shown;
            let id = self.param_instances[base];
            let rt = self.editor.runtime_mut();
            let _ = rt.set_instance_field(id, "mod-value", Value::Number(shown.0 as f64));
            let _ = rt.set_instance_field(id, "mod-offset", Value::Number(shown.1 as f64));
            let _ = rt.set_instance_field(id, "mod-scale", Value::Number(shown.2 as f64));
            let ratio = if spec.percent { shown.0 / 100.0 } else { shown.0 };
            let _ = rt.set_instance_field(id, "mod-ratio", Value::Number(ratio as f64));
            changed = true;
        }
        changed
    }

    /// Turn the panel's host commands into edits.
    fn drain_commands(&mut self) {
        for command in self.editor.drain_host_commands() {
            let HostCommand::Custom { name, payload } = command else { continue };
            if self.trace {
                crate::shell::log(&format!("host-command {name} {payload:?}"));
            }
            match name.as_str() {
                "set-instrument-param" | "set-instrument-plock" | "set-instrument-key-lock-multi"
                | "set-device-param" => {
                    self.edit_from(&payload);
                }
                "set-instrument-param-batch" | "set-instrument-plock-batch" => {
                    if let Some(Value::List(updates)) = dict_get(&payload, "updates") {
                        for update in updates {
                            let update = update.borrow().clone();
                            self.edit_from(&update);
                        }
                    }
                    if dict_get(&payload, "commit").is_some_and(|c| !matches!(c, Value::Bool(false) | Value::Nil)) {
                        self.edits.push(PanelEdit::Commit);
                    }
                }
                "toggle-instrument-param" => {
                    if let Some(index) = number(dict_get(&payload, "param-idx")).map(|i| i as usize) {
                        if let Some(spec) = self.spec.params.get(index) {
                            let next = if spec.kind == ParamKind::Boolean {
                                if self.params.get(index) > 0.5 { 0.0 } else { 1.0 }
                            } else if self.params.get(index) > spec.min {
                                spec.min
                            } else {
                                spec.max
                            };
                            self.push_stored(index, next);
                            self.edits.push(PanelEdit::Commit);
                        }
                    }
                }
                "set-instrument-param-option" | "set-instrument-plock-option" => {
                    let index = number(dict_get(&payload, "param-idx")).map(|i| i as usize);
                    let label = match dict_get(&payload, "label") {
                        Some(Value::String(label)) => label,
                        _ => continue,
                    };
                    if let Some(index) = index.filter(|&i| i < self.spec.params.len()) {
                        if let Some(at) = self.spec.params[index].options.iter().position(|o| *o == label) {
                            self.push_stored(index, at as f32);
                            self.edits.push(PanelEdit::Commit);
                        }
                    }
                }
                _ => {
                    // Panel features with no plugin counterpart (sequencing,
                    // racks, tensors): say so once rather than fail silently.
                    if self.unsupported_logged.insert(name.clone()) {
                        crate::shell::log(&format!("panel command not supported in the plugin: {name}"));
                    }
                }
            }
        }
    }

    fn edit_from(&mut self, payload: &Value) {
        let (Some(index), Some(value)) = (
            number(dict_get(payload, "param-idx")).map(|i| i as usize),
            number(dict_get(payload, "value")),
        ) else {
            return;
        };
        if let Some(spec) = self.spec.params.get(index) {
            let stored = spec.from_display(value as f32);
            self.push_stored(index, stored);
        }
    }

    /// The panel owns the store write; the editor only tells the host.
    fn push_stored(&mut self, index: usize, value: f32) {
        let value = self.spec.params[index].clamp(value);
        self.params.set(index, value);
        self.edits.push(PanelEdit::Value(index, value));
    }

    /// Edits made since the last call.
    pub fn take_edits(&mut self) -> Vec<PanelEdit> {
        std::mem::take(&mut self.edits)
    }

    /// One frame: pull host changes in, push panel edits out, draw if needed.
    /// Settle input and host changes: apply the coalesced drag, mirror the
    /// store, run the runtime's side effects (only when something happened)
    /// and turn the panel's host commands into edits.
    fn pump(&mut self) {
        if let Some((event, col, row)) = self.pending_drag.take() {
            self.editor.handle_tiled_mouse_precise(event, col, row, 0);
            self.dirty = true;
        }
        self.sync_from_store();
        if std::mem::take(&mut self.dirty) {
            self.editor.refresh_runtime_side_effects();
            self.drain_commands();
        }
    }

    pub fn frame(&mut self) {
        self.pump();
        // The tab changed the panel's size: hold the last frame until the
        // host applies the new window size (or ignores the request), so the
        // new layout never shows squeezed into the old window.
        let wanted = self.wanted_size();
        if wanted != self.size {
            let since = *self.resize_wait.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() < RESIZE_WAIT {
                return;
            }
        } else {
            self.resize_wait = None;
        }
        let animating = self.editor.visible_widgets_animating()
            || eseqlisp::widget_render::sdf_widget::sdf_visual_animations_active(self.backend.time_seconds());
        if self.editor.needs_redraw() || animating {
            // The tiled path (the app's) honours the tile's hidden status
            // line, and is what the pointer handlers hit-test against.
            let (cols, rows) = self.grid();
            let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(&mut self.editor, cols, rows);
            match self.backend.render_tiled(&tiled) {
                Ok(eseqlisp::metal_backend::TiledRenderStatus::Presented) => self.editor.clear_needs_redraw(),
                _ => eseqlisp::frame::requeue_unpresented_tiled_frame(&mut self.editor, &tiled),
            }
        }
    }

    /// Every laid-out widget of `widget_type`: (stable key, rect in points).
    pub fn widget_rects(&self, widget_type: &str) -> Vec<(String, (f64, f64, f64, f64))> {
        fn walk(node: &eseqlisp::layout::LayoutNode, ty: &str, out: &mut Vec<(String, eseqlisp::layout::Rect)>) {
            if node.widget_type == ty {
                out.push((node.stable_key.clone().unwrap_or_default(), node.rect));
            }
            for child in &node.children {
                walk(child, ty, out);
            }
        }
        let mut rects = Vec::new();
        if let Some(layout) = self.editor.widget_layout() {
            walk(&layout, widget_type, &mut rects);
        }
        let (cw, ch) = self.cell;
        let to_pt = |cells: f32, cell: f32| (cells * cell) as f64 / self.scale;
        rects
            .into_iter()
            .map(|(key, r)| (key, (to_pt(r.col, cw), to_pt(r.row, ch), to_pt(r.width, cw), to_pt(r.height, ch))))
            .collect()
    }

    /// Invoke the first `widget_type` widget's `prop` callback with `args`
    /// (headless tests).
    pub fn invoke_first(&mut self, widget_type: &str, prop: &str, args: Vec<Value>) -> Result<(), String> {
        self.invoke_keyed(widget_type, "", prop, args)
    }

    /// As [`Self::invoke_first`], for the first whose stable key ends with
    /// `key_suffix`.
    pub fn invoke_keyed(&mut self, widget_type: &str, key_suffix: &str, prop: &str, args: Vec<Value>) -> Result<(), String> {
        fn find(node: &eseqlisp::layout::LayoutNode, ty: &str, suffix: &str) -> Option<eseqlisp::layout::LayoutNode> {
            if node.widget_type == ty && node.stable_key.as_deref().unwrap_or("").ends_with(suffix) {
                return Some(node.clone());
            }
            node.children.iter().find_map(|c| find(c, ty, suffix))
        }
        let layout = self.editor.widget_layout().ok_or("no layout")?;
        let node = find(&layout, widget_type, key_suffix).ok_or("no such widget")?;
        let callback = node.props.get(prop).cloned().ok_or("no such prop")?;
        self.editor.runtime_mut().invoke(callback, args).map_err(|e| format!("{e:?}"))?;
        self.dirty = true;
        Ok(())
    }

    /// A frame's host traffic without drawing (headless tests).
    pub fn frame_headless(&mut self) {
        self.pump();
    }

    /// Render the panel to a PNG through the live (tiled) renderer, into an
    /// offscreen target: exactly what the plugin window shows.
    pub fn render_png(&mut self, path: &Path) -> Result<(), String> {
        self.pump();
        let (cols, rows) = self.grid();
        let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(&mut self.editor, cols, rows);
        let w = (self.size.0 * self.scale).round() as u32;
        let h = (self.size.1 * self.scale).round() as u32;
        let target = self.backend.create_tiled_capture_target(w, h).map_err(|_| "capture target")?;
        self.backend.render_tiled_capture(&tiled, &target).map_err(|_| "render failed")?;
        image::save_buffer(path, &target.rgba(), w, h, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Point → cell coordinates.
    pub fn cell_at(&self, x: f64, y: f64) -> (f32, f32) {
        let (cell_w, cell_h) = self.cell;
        ((x * self.scale) as f32 / cell_w, (y * self.scale) as f32 / cell_h)
    }

    /// A mouse event at point (x, y) from the top-left of the view. Drags
    /// are coalesced to one per frame; a release applies the pending one
    /// first so the final position lands.
    pub fn mouse(&mut self, kind: crossterm::event::MouseEventKind, x: f64, y: f64, modifiers: crossterm::event::KeyModifiers) {
        use crossterm::event::MouseEventKind;
        let (col, row) = self.cell_at(x, y);
        let event = crossterm::event::MouseEvent {
            kind,
            column: col.max(0.0) as u16,
            row: row.max(0.0) as u16,
            modifiers,
        };
        if matches!(kind, MouseEventKind::Drag(_)) {
            self.pending_drag = Some((event, col, row));
            return;
        }
        if let Some((drag, drag_col, drag_row)) = self.pending_drag.take() {
            self.editor.handle_tiled_mouse_precise(drag, drag_col, drag_row, 0);
        }
        self.editor.handle_tiled_mouse_precise(event, col, row, 0);
        self.dirty = true;
        if matches!(kind, MouseEventKind::Up(_)) {
            self.edits.push(PanelEdit::Commit);
        }
    }

    /// Whether the panel takes `key` rather than the host: only while a
    /// focused widget handles it — typing into a focused knob or number
    /// (digits, sign, point, Enter, Backspace, Escape), or a key the focused
    /// widget's own handler consumes. Everything else (space to play, the
    /// host's shortcuts) goes back to the host.
    pub fn wants_key(&self, key: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;
        if self.editor.focused_widget_captures_numeric_input()
            && matches!(
                key.code,
                KeyCode::Char('0'..='9' | '.' | '-' | '+' | ',')
                    | KeyCode::Enter
                    | KeyCode::Backspace
                    | KeyCode::Delete
                    | KeyCode::Esc
                    | KeyCode::Left
                    | KeyCode::Right
            )
        {
            return true;
        }
        !matches!(key.code, KeyCode::Char(_) | KeyCode::Enter) && self.editor.focused_widget_consumes_key(key.code, key.modifiers)
    }

    pub fn key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        self.editor.handle_key(key);
        self.dirty = true;
        // Enter commits a typed value, Escape abandons it: either closes the
        // host gesture the edit opened.
        if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
            self.pump();
            self.edits.push(PanelEdit::Commit);
        }
        self.editor.mark_needs_redraw();
    }

    pub fn scroll(&mut self, x: f64, y: f64, dx: f32, dy: f32) {
        let (col, row) = self.cell_at(x, y);
        self.editor.handle_tiled_touchpad_scroll(col, row, 0, dx, dy);
        self.dirty = true;
    }
}

/// Process-wide eseqlisp roots. Every instance of one plugin shares the image
/// and points them at the same payload, so re-setting them on each boot is
/// harmless.
fn configure_roots(content: &Path) {
    // The live shader-override watch reads this crate's source tree, which
    // only exists in a checkout; a plugin is always packaged.
    eseqlisp::ui::set_editable_shader_overrides_enabled(false);
    eseqlisp::hot_reload::set_global_load_fallback_roots(vec![content.to_path_buf()]);
    let assets: PathBuf = content.join("assets");
    eseqlisp::widget_render::patcher::set_asset_roots(vec![content.to_path_buf()], assets.clone(), assets);
}

fn eval(rt: &mut Runtime, source: &str, what: &str) -> Result<(), String> {
    rt.eval_str(source).map_err(|e| format!("{what}: {e:?}"))?;
    let errors = rt.take_source_load_errors();
    if !errors.is_empty() {
        return Err(format!("{what}: {}", errors.join("; ")));
    }
    Ok(())
}

/// The app natives and globals a panel can reach that a plugin has no
/// counterpart for: no step selection (so edits are base values, never
/// p-locks), no bus, no MIDI/audio effect panels.
const PLUGIN_STUBS: &str = r#"
(def seq-has-selection? () false)
(def custom-midi-fx-ui (fx) false)
(def custom-audio-fx-ui (fx) false)
"#;

/// The plugin's one buffer: the current track's instrument panel, as the
/// *fx* buffer shows it, without the device chain around it.
const TOGGLE_MODS: &str = "(eseq.effects.effect-panels/instrument-toggle-mods-view)";
/// The kind holding the instrument panel's tab state (`mods-open`).
const INSTRUMENT_VIEW_KIND: &str = "eseq.effects.state:instrument-view";

const PLUGIN_ROOT: &str = r#"
(effect-buffer "*plugin*"
  (let ((inst (eseq.effects.panel-data/current-instrument-panel)))
    (if inst
      (box :debug-name "plugin-root" :padding 0.5 :h-align :start :v-align :start
        (v-stack :gap 0.3 :align :start
          (h-stack :debug-name "plugin-header" :gap 1 :height 1.0 :align :center
            (eseq.effects.effect-panels/instrument-synth-button)
            (eseq.effects.effect-panels/instrument-mods-toggle-button)
            (let ((v (plugin-voices-param)))
              (if v
                (h-stack :gap 0.3 :height 0.8 :align :center
                  (label "voices" :font-size 9 :color :dim :bg :transparent)
                  (number-picker :key "plugin-voices" :noui true :width 2.5 :height 0.8
                    :font-size 10 :decimals 0 :step 1 :min v.min :max v.max
                    :value #'v.value
                    :on-change (lambda (x) (host-command "set-instrument-param"
                                             (dict :param-idx v.index :value x)))))
                (box :width 0 :height 0))))
          (eseq.effects.panel-bodies/instrument-synth-panel-body inst)))
      (label "this instrument has no panel"))))
"#;

/// The kinds the instrument panel lays out from: track 0 (selected), its
/// instrument device, one param per descriptor param (in the app's order, so
/// a `param-idx` means the same thing), the modulation lanes and the
/// modulators.
struct PublishedKinds {
    device: InstanceId,
    params: Vec<InstanceId>,
}

fn publish_kinds(rt: &mut Runtime, spec: &PluginSpec, params: &ParamStore) -> Result<PublishedKinds, String> {
    let kind = |k: &str| format!("eseq.kinds:{k}");
    let track = rt.register_keyed_instance(&kind("track"), &[0]).map_err(|e| format!("track: {e:?}"))?;
    set(rt, track, "index", Value::Number(0.0))?;
    set(rt, track, "tid", Value::Number(0.0))?;
    set(rt, track, "selected", Value::Bool(true))?;
    set(rt, track, "name", Value::String(spec.name.clone()))?;
    let project = rt.singleton_instance(&kind("project")).ok_or("no project kind")?;
    set(rt, project, "tracks", list(vec![Value::Instance(track)]))?;
    let selection = rt.singleton_instance(&kind("selection")).ok_or("no selection kind")?;
    set(rt, selection, "track", Value::Instance(track))?;
    set(rt, selection, "rack-slot", Value::Number(-1.0))?;

    let device = rt
        .register_keyed_instance(&kind("device"), &[track, 0])
        .map_err(|e| format!("device: {e:?}"))?;
    let name = spec.source_instrument.clone();
    for (field, value) in [
        ("track", Value::Instance(track)),
        ("slot", Value::Number(-1.0)),
        ("did", Value::Number(0.0)),
        ("role", Value::String("instrument".into())),
        ("name", Value::String(name.clone())),
        ("type", Value::String(name.clone())),
        ("instrument-name", Value::String(name.clone())),
        ("display-name", Value::String(spec.name.clone())),
        ("meter", Value::Nil),
        ("builtin", Value::Bool(false)),
        ("enabled", Value::Bool(true)),
        ("node-id", Value::Number(1.0)),
        ("tensors", list(Vec::new())),
        ("mod-phases", list(vec![Value::Number(-1.0); vm::SLOT_COUNT])),
    ] {
        set(rt, device, field, value)?;
    }
    let modulators = spec
        .dsp
        .layout
        .modulators
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let id = rt
                .register_keyed_instance(&kind("modulator"), &[device, index as u64])
                .map_err(|e| format!("modulator: {e:?}"))?;
            set(rt, id, "device", Value::Instance(device))?;
            set(rt, id, "index", Value::Number(index as f64))?;
            set(rt, id, "slot", Value::Number(m.slot as f64))?;
            set(rt, id, "label", Value::String(format!("Mod {}", m.slot)))?;
            set(rt, id, "ext", Value::Number(m.ext.unwrap_or(0) as f64))?;
            Ok(Value::Instance(id))
        })
        .collect::<Result<Vec<_>, String>>()?;
    set(rt, device, "modulators", list(modulators))?;

    let mut ids = Vec::with_capacity(spec.params.len());
    for (index, p) in spec.params.iter().enumerate() {
        let id = rt
            .register_keyed_instance(&kind("param"), &[device, index as u64])
            .map_err(|e| format!("param {}: {e:?}", p.name))?;
        let stored = params.get(index);
        let value = p.to_display(stored) as f64;
        let options: Vec<Value> = p.options.iter().map(|o| Value::String(o.clone())).collect();
        let text = p.option_label(stored).unwrap_or_default().to_string();
        let type_name = match p.kind {
            ParamKind::Continuous => "continuous",
            ParamKind::Enum => "enum",
            ParamKind::Boolean => "boolean",
        };
        for (field, v) in [
            ("device", Value::Instance(device)),
            ("index", Value::Number(index as f64)),
            ("name", Value::String(p.name.clone())),
            ("label", Value::String(p.label.clone())),
            ("display-name", Value::String(p.label.clone())),
            ("section", Value::String(p.section.clone())),
            ("mod-slot", Value::Number(p.mod_slot as f64)),
            ("visible", Value::Bool(p.section != "hidden")),
            ("type", Value::String(type_name.into())),
            ("options", list(options)),
            ("asset-options", Value::Nil),
            ("text", Value::String(text)),
            ("unit", Value::String(p.unit.clone())),
            ("percent", Value::Bool(p.percent)),
            ("group", Value::String(String::new())),
            ("env", Value::String(String::new())),
            ("role", Value::String(String::new())),
            ("host-modulatable", Value::Bool(false)),
            ("min", Value::Number(p.to_display(p.min) as f64)),
            ("max", Value::Number(p.to_display(p.max) as f64)),
            ("default", Value::Number(p.to_display(p.default) as f64)),
            ("value", Value::Number(value)),
            ("base", Value::Number(value)),
            ("process-value", Value::Number(value)),
            ("mod-value", Value::Number(value)),
            ("mod-ratio", Value::Number(stored as f64)),
            ("mod-scale", Value::Number(1.0)),
            ("mod-offset", Value::Number(0.0)),
            ("mod-phase", Value::Number(-1.0)),
            ("mod-targets", list(Vec::new())),
            ("key-locks", list(Vec::new())),
            ("step-locks", list(Vec::new())),
        ] {
            set(rt, id, field, v)?;
        }
        ids.push(id);
    }
    // Each depth lane, on its base param, in lane order.
    let mut lanes_of: Vec<Vec<Value>> = vec![Vec::new(); spec.params.len()];
    for target in &spec.dsp.layout.mod_targets {
        let lane = lanes_of[target.base].len();
        let id = rt
            .register_keyed_instance(&kind("mod-target"), &[ids[target.base], lane as u64])
            .map_err(|e| format!("mod-target: {e:?}"))?;
        for (field, v) in [
            ("param", Value::Instance(ids[target.base])),
            ("index", Value::Number(lane as f64)),
            ("slot", Value::Number(target.slot as f64)),
            ("source", Value::Nil),
            ("depth", Value::Instance(ids[target.depth])),
            ("depth-min", Value::Number(target.depth_min as f64)),
            ("depth-max", Value::Number(target.depth_max as f64)),
            ("unit", Value::String(target.unit.clone())),
        ] {
            set(rt, id, field, v)?;
        }
        lanes_of[target.base].push(Value::Instance(id));
    }
    for (index, lanes) in lanes_of.into_iter().enumerate() {
        if !lanes.is_empty() {
            set(rt, ids[index], "mod-targets", list(lanes))?;
        }
    }
    set(rt, device, "params", list(ids.iter().map(|&id| Value::Instance(id)).collect()))?;
    set(rt, track, "devices", list(vec![Value::Instance(device)]))?;
    rt.run_reactive_cycle();
    Ok(PublishedKinds { device, params: ids })
}

/// The cells the panel occupies, from the layout's top-left: the furthest
/// extent of the root's content (header + panel body, the mods column when
/// open).
fn content_extent(root: &eseqlisp::layout::LayoutNode) -> (f32, f32) {
    fn find<'a>(node: &'a eseqlisp::layout::LayoutNode, name: &str) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if matches!(node.props.get("debug-name"), Some(Value::String(v)) if v == name) {
            return Some(node);
        }
        node.children.iter().find_map(|child| find(child, name))
    }
    fn extent(node: &eseqlisp::layout::LayoutNode) -> (f32, f32) {
        node.children.iter().map(extent).fold(
            (node.rect.col + node.rect.width, node.rect.row + node.rect.height),
            |(r, b), (cr, cb)| (r.max(cr), b.max(cb)),
        )
    }
    let content = find(root, "plugin-root").and_then(|r| r.children.first()).unwrap_or(root);
    extent(content)
}
