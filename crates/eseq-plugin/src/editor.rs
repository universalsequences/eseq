//! The plugin's editor: the instrument's own panel ([`PanelHost`]) in an
//! NSView this module defines, parented into the host's plugin window, driven
//! by a main-thread timer. Exposed to the host as the CLAP `gui` extension
//! (clap-wrapper maps it to the VST3 `IPlugView`).
//!
//! The view class is registered at runtime under a name unique to this
//! image: every exported plugin is a copy of the same shell, so a fixed class
//! name would collide in a host that loads two of them.

use std::collections::BTreeSet;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, OnceLock};

use clap_sys::ext::gui::*;
use clap_sys::plugin::clap_plugin;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{ClassType, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::panel::{PanelEdit, PanelHost};
use crate::shell::{GuiEdit, Shared};

/// Frames per second the panel is driven at while open.
const FRAME_RATE: f64 = 60.0;
const STATE_IVAR: &CStr = c"eseqPanelState";

/// What the view's methods reach through its ivar.
struct ViewState {
    panel: PanelHost,
    shared: Arc<Shared>,
    /// Params with a host gesture open (between the first edit of a drag and
    /// its release).
    gestures: BTreeSet<usize>,
}

impl ViewState {
    /// The panel's edits, as host gestures: the first value of a param opens
    /// its gesture, a commit closes every open one.
    fn forward_edits(&mut self) {
        let mut out = Vec::new();
        for edit in self.panel.take_edits() {
            match edit {
                PanelEdit::Value(index, value) => {
                    if self.gestures.insert(index) {
                        out.push(GuiEdit::Begin(index));
                    }
                    out.push(GuiEdit::Value(index, value));
                }
                PanelEdit::Commit => {
                    out.extend(std::mem::take(&mut self.gestures).into_iter().map(GuiEdit::End));
                }
            }
        }
        if !out.is_empty() {
            self.shared.send_edits(out);
        }
    }
}

pub struct Editor {
    /// Booted at `create`, moved into the view at `set_parent`.
    pending: Option<PanelHost>,
    view: Option<Retained<NSView>>,
    timer: Option<Retained<AnyObject>>,
    size: (f64, f64),
}

impl Editor {
    fn state(&self) -> Option<&mut ViewState> {
        let view = self.view.as_ref()?;
        unsafe { state_of(view) }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        unsafe {
            if let Some(timer) = self.timer.take() {
                let _: () = msg_send![&*timer, invalidate];
            }
            if let Some(view) = self.view.take() {
                let _: () = msg_send![&*view, removeFromSuperview];
                if let Some(ivar) = view.class().instance_variable(STATE_IVAR) {
                    let slot = ivar.load_ptr::<*mut c_void>(&view);
                    let state = std::mem::replace(&mut *slot, std::ptr::null_mut());
                    if !state.is_null() {
                        drop(Box::from_raw(state as *mut ViewState));
                    }
                }
            }
        }
    }
}

unsafe fn state_of<'a>(view: &AnyObject) -> Option<&'a mut ViewState> {
    let ivar = view.class().instance_variable(STATE_IVAR)?;
    let ptr = unsafe { *ivar.load::<*mut c_void>(view) } as *mut ViewState;
    unsafe { ptr.as_mut() }
}

/// The view class, registered once per image under a unique name.
fn view_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let class = *CLASS.get_or_init(|| {
        let unique = format!("EseqPluginPanelView_{:x}_{:x}", std::process::id(), view_class as *const () as usize);
        let name = CString::new(unique).unwrap();
        let mut builder = ClassBuilder::new(&name, NSView::class()).expect("unique view class");
        builder.add_ivar::<*mut c_void>(STATE_IVAR);
        unsafe {
            builder.add_method(sel!(isFlipped), yes as extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool);
            builder.add_method(sel!(acceptsFirstResponder), yes as extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool);
            builder.add_method(
                sel!(acceptsFirstMouse:),
                yes_with as extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool,
            );
            type Handler = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);
            builder.add_method(sel!(mouseDown:), mouse_down as Handler);
            builder.add_method(sel!(mouseDragged:), mouse_dragged as Handler);
            builder.add_method(sel!(mouseUp:), mouse_up as Handler);
            builder.add_method(sel!(rightMouseDown:), right_down as Handler);
            builder.add_method(sel!(rightMouseUp:), right_up as Handler);
            builder.add_method(sel!(mouseMoved:), mouse_moved as Handler);
            builder.add_method(sel!(scrollWheel:), scroll_wheel as Handler);
            builder.add_method(sel!(eseqTick:), tick as Handler);
            builder.add_method(sel!(keyDown:), key_down as Handler);
            builder.add_method(
                sel!(updateTrackingAreas),
                update_tracking_areas as extern "C-unwind" fn(*mut AnyObject, Sel),
            );
        }
        builder.register() as *const AnyClass as usize
    });
    unsafe { &*(class as *const AnyClass) }
}

extern "C-unwind" fn yes(_: *mut AnyObject, _: Sel) -> Bool {
    Bool::YES
}

extern "C-unwind" fn yes_with(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
    Bool::YES
}

/// The event's position in the view's (flipped, top-left) points.
unsafe fn location(view: &AnyObject, event: *mut AnyObject) -> (f64, f64) {
    unsafe {
        let in_window: NSPoint = msg_send![event, locationInWindow];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let point: NSPoint = msg_send![view, convertPoint: in_window, fromView: nil];
        (point.x, point.y)
    }
}

// NSEventModifierFlags.
const NS_SHIFT: usize = 1 << 17;
const NS_CONTROL: usize = 1 << 18;
const NS_OPTION: usize = 1 << 19;
const NS_COMMAND: usize = 1 << 20;
// NSTrackingAreaOptions.
const NS_TRACKING_MOUSE_ENTERED_AND_EXITED: usize = 0x01;
const NS_TRACKING_MOUSE_MOVED: usize = 0x02;
const NS_TRACKING_ACTIVE_ALWAYS: usize = 0x80;
const NS_TRACKING_IN_VISIBLE_RECT: usize = 0x200;

unsafe fn modifiers(event: *mut AnyObject) -> KeyModifiers {
    let flags: usize = unsafe { msg_send![event, modifierFlags] };
    let mut mods = KeyModifiers::NONE;
    for (flag, modifier) in [
        (NS_SHIFT, KeyModifiers::SHIFT),
        (NS_CONTROL, KeyModifiers::CONTROL),
        (NS_OPTION, KeyModifiers::ALT),
        (NS_COMMAND, KeyModifiers::SUPER),
    ] {
        if flags & flag != 0 {
            mods |= modifier;
        }
    }
    mods
}

fn mouse(view: &AnyObject, event: *mut AnyObject, kind: MouseEventKind) {
    unsafe {
        let Some(state) = state_of(view) else { return };
        let (x, y) = location(view, event);
        state.panel.mouse(kind, x, y, modifiers(event));
        state.forward_edits();
    }
}

extern "C-unwind" fn mouse_down(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    let view = unsafe { &*view };
    // Take key focus, so a clicked knob can be typed into.
    unsafe {
        let window: *mut AnyObject = msg_send![view, window];
        if !window.is_null() {
            let _: Bool = msg_send![window, makeFirstResponder: view];
        }
    }
    mouse(view, event, MouseEventKind::Down(MouseButton::Left));
}

extern "C-unwind" fn mouse_dragged(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    mouse(unsafe { &*view }, event, MouseEventKind::Drag(MouseButton::Left));
}

extern "C-unwind" fn mouse_up(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    let view = unsafe { &*view };
    mouse(view, event, MouseEventKind::Up(MouseButton::Left));
    follow_panel_size(view);
}

extern "C-unwind" fn right_down(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    mouse(unsafe { &*view }, event, MouseEventKind::Down(MouseButton::Right));
}

extern "C-unwind" fn right_up(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    mouse(unsafe { &*view }, event, MouseEventKind::Up(MouseButton::Right));
}

extern "C-unwind" fn mouse_moved(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    mouse(unsafe { &*view }, event, MouseEventKind::Moved);
}

extern "C-unwind" fn scroll_wheel(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    unsafe {
        let view = &*view;
        let Some(state) = state_of(view) else { return };
        let (x, y) = location(view, event);
        let dx: f64 = msg_send![event, scrollingDeltaX];
        let dy: f64 = msg_send![event, scrollingDeltaY];
        let precise: Bool = msg_send![event, hasPreciseScrollingDeltas];
        let scale = if precise.as_bool() { 1.0 } else { 10.0 };
        state.panel.scroll(x, y, (dx * scale) as f32, (dy * scale) as f32);
        state.forward_edits();
    }
}

/// A key the panel takes (typing into a focused knob, …) goes to the editor;
/// any other continues up the responder chain to the host.
extern "C-unwind" fn key_down(view: *mut AnyObject, _: Sel, event: *mut AnyObject) {
    unsafe {
        let view = &*view;
        if let (Some(state), Some(key)) = (state_of(view), key_event(event)) {
            if state.panel.wants_key(&key) {
                state.panel.key(key);
                state.panel.frame();
                state.forward_edits();
                return;
            }
        }
        let _: () = msg_send![super(view, NSView::class()), keyDown: event];
    }
}

/// An NSEvent key press as the crossterm event eseq's own window produces.
unsafe fn key_event(event: *mut AnyObject) -> Option<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyCode, KeyEvent};
    let code: u16 = unsafe { msg_send![event, keyCode] };
    let modifiers = unsafe { modifiers(event) };
    let named = match code {
        36 | 76 => Some(KeyCode::Enter),
        48 => Some(KeyCode::Tab),
        51 => Some(KeyCode::Backspace),
        117 => Some(KeyCode::Delete),
        53 => Some(KeyCode::Esc),
        123 => Some(KeyCode::Left),
        124 => Some(KeyCode::Right),
        125 => Some(KeyCode::Down),
        126 => Some(KeyCode::Up),
        115 => Some(KeyCode::Home),
        119 => Some(KeyCode::End),
        _ => None,
    };
    let code = match named {
        Some(code) => code,
        None => {
            let chars: Option<Retained<objc2_foundation::NSString>> =
                unsafe { msg_send![event, charactersIgnoringModifiers] };
            let ch = chars?.to_string().chars().next()?;
            if ch.is_control() {
                return None;
            }
            KeyCode::Char(ch)
        }
    };
    // Shift is already in the character (`+`, capitals).
    let modifiers = if matches!(code, KeyCode::Char(_)) { modifiers - KeyModifiers::SHIFT } else { modifiers };
    Some(KeyEvent::new(code, modifiers))
}

/// Hover (mouseMoved:) needs a tracking area covering the view.
extern "C-unwind" fn update_tracking_areas(view: *mut AnyObject, _: Sel) {
    unsafe {
        let view = &*view;
        let areas: *mut AnyObject = msg_send![view, trackingAreas];
        let count: usize = msg_send![areas, count];
        for i in (0..count).rev() {
            let area: *mut AnyObject = msg_send![areas, objectAtIndex: i];
            let _: () = msg_send![view, removeTrackingArea: area];
        }
        let bounds: NSRect = msg_send![view, bounds];
        let options = NS_TRACKING_MOUSE_ENTERED_AND_EXITED
            | NS_TRACKING_MOUSE_MOVED
            | NS_TRACKING_ACTIVE_ALWAYS
            | NS_TRACKING_IN_VISIBLE_RECT;
        let Some(class) = AnyClass::get(c"NSTrackingArea") else { return };
        let area: *mut AnyObject = msg_send![class, alloc];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let area: *mut AnyObject =
            msg_send![area, initWithRect: bounds, options: options, owner: view, userInfo: nil];
        let _: () = msg_send![view, addTrackingArea: area];
        let _: () = msg_send![area, release];
    }
}

extern "C-unwind" fn tick(view: *mut AnyObject, _: Sel, _timer: *mut AnyObject) {
    unsafe {
        let view = &*view;
        let hidden: Bool = msg_send![view, isHiddenOrHasHiddenAncestor];
        if hidden.as_bool() {
            return;
        }
        let Some(state) = state_of(view) else { return };
        state.panel.frame();
        state.forward_edits();
    }
}

/// After a click: when the mods tab opened or closed, ask the host for the
/// window size that fits it (it answers through `set_size`).
fn follow_panel_size(view: &AnyObject) {
    let Some(state) = (unsafe { state_of(view) }) else { return };
    let wanted = state.panel.wanted_size();
    if wanted != state.panel.size() {
        state.shared.request_resize(wanted.0.ceil() as u32, wanted.1.ceil() as u32);
    }
}

fn main_screen_scale() -> f64 {
    unsafe {
        let Some(class) = AnyClass::get(c"NSScreen") else { return 2.0 };
        let screen: *mut AnyObject = msg_send![class, mainScreen];
        if screen.is_null() {
            return 2.0;
        }
        let scale: f64 = msg_send![screen, backingScaleFactor];
        if scale > 0.0 { scale } else { 2.0 }
    }
}

// ── CLAP gui extension ──────────────────────────────────────────────────

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: Some(gui_get_resize_hints),
    adjust_size: Some(gui_adjust_size),
    set_size: Some(gui_set_size),
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

pub fn gui_extension() -> *const c_void {
    &GUI as *const clap_plugin_gui as *const c_void
}

fn is_cocoa(api: *const c_char) -> bool {
    !api.is_null() && unsafe { CStr::from_ptr(api) } == CLAP_WINDOW_API_COCOA
}

unsafe extern "C" fn gui_is_api_supported(plugin: *const clap_plugin, api: *const c_char, is_floating: bool) -> bool {
    let has_panel = unsafe { crate::shell::shared_of(plugin) }.shell.payload.spec.ui.is_some();
    has_panel && is_cocoa(api) && !is_floating
}

unsafe extern "C" fn gui_get_preferred_api(_: *const clap_plugin, api: *mut *const c_char, is_floating: *mut bool) -> bool {
    unsafe {
        if !api.is_null() {
            *api = CLAP_WINDOW_API_COCOA.as_ptr();
        }
        if !is_floating.is_null() {
            *is_floating = false;
        }
    }
    true
}

unsafe extern "C" fn gui_create(plugin: *const clap_plugin, api: *const c_char, is_floating: bool) -> bool {
    if !is_cocoa(api) || is_floating {
        return false;
    }
    let slot = unsafe { crate::shell::editor_slot(plugin) };
    let Ok(mut editor) = slot.try_borrow_mut() else { return false };
    if editor.is_some() {
        return true;
    }
    let shared = unsafe { crate::shell::shared_of(plugin) };
    let started = std::time::Instant::now();
    match PanelHost::boot(&shared.shell.payload, shared.params.clone(), shared.mod_display.clone(), main_screen_scale()) {
        Ok(panel) => {
            crate::shell::log(&format!("panel booted in {:?}", started.elapsed()));
            let size = panel.size();
            *editor = Some(Editor { pending: Some(panel), view: None, timer: None, size });
            true
        }
        Err(err) => {
            crate::shell::log(&format!("panel failed to boot: {err}"));
            false
        }
    }
}

unsafe extern "C" fn gui_destroy(plugin: *const clap_plugin) {
    let editor = unsafe { crate::shell::editor_slot(plugin) }.try_borrow_mut().map(|mut slot| slot.take());
    // Drop outside the borrow: tearing the view down may re-enter the host.
    drop(editor);
}

unsafe extern "C" fn gui_set_scale(_: *const clap_plugin, _scale: f64) -> bool {
    // macOS sizes are in points; the backing scale comes from the screen.
    false
}

unsafe extern "C" fn gui_get_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    let Ok(slot) = unsafe { crate::shell::editor_slot(plugin) }.try_borrow() else { return false };
    let Some(editor) = slot.as_ref() else { return false };
    if width.is_null() || height.is_null() {
        return false;
    }
    unsafe {
        *width = editor.size.0.ceil() as u32;
        *height = editor.size.1.ceil() as u32;
    }
    true
}

// The window has one size per panel tab (synth, mods). Saying it can
// resize is what lets a host (and clap-wrapper's VST3 view, whose onSize only
// forwards to resizable plugins) apply the size the panel asks for; the
// hints forbid free dragging and `adjust_size` snaps to the tab's size.
unsafe extern "C" fn gui_can_resize(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn gui_get_resize_hints(_: *const clap_plugin, hints: *mut clap_gui_resize_hints) -> bool {
    let Some(hints) = (unsafe { hints.as_mut() }) else { return false };
    hints.can_resize_horizontally = false;
    hints.can_resize_vertically = false;
    hints.preserve_aspect_ratio = false;
    hints.aspect_ratio_width = 1;
    hints.aspect_ratio_height = 1;
    true
}

unsafe extern "C" fn gui_adjust_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    let Ok(mut slot) = unsafe { crate::shell::editor_slot(plugin) }.try_borrow_mut() else { return false };
    let Some(editor) = slot.as_mut() else { return false };
    let wanted = if let Some(panel) = editor.pending.as_mut() {
        panel.wanted_size()
    } else if let Some(state) = editor.state() {
        state.panel.wanted_size()
    } else {
        editor.size
    };
    unsafe {
        if let (Some(w), Some(h)) = (width.as_mut(), height.as_mut()) {
            *w = wanted.0.ceil() as u32;
            *h = wanted.1.ceil() as u32;
        }
    }
    true
}

/// The host's answer to `request_resize` (or its own resize): the view, its
/// layer and the panel's grid follow.
unsafe extern "C" fn gui_set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
    let Ok(mut slot) = unsafe { crate::shell::editor_slot(plugin) }.try_borrow_mut() else { return false };
    let Some(editor) = slot.as_mut() else { return false };
    let size = (width as f64, height as f64);
    editor.size = size;
    if let Some(panel) = editor.pending.as_mut() {
        panel.resize(size.0, size.1);
    }
    if let Some(view) = &editor.view {
        unsafe {
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size.0, size.1));
            let _: () = msg_send![&**view, setFrame: frame];
        }
    }
    if let Some(state) = editor.state() {
        state.panel.resize(size.0, size.1);
    }
    true
}

unsafe extern "C" fn gui_set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
    let shared = unsafe { crate::shell::shared_of(plugin) }.clone();
    let Ok(mut slot) = unsafe { crate::shell::editor_slot(plugin) }.try_borrow_mut() else { return false };
    let Some(editor) = slot.as_mut() else { return false };
    let Some(window) = (unsafe { window.as_ref() }) else { return false };
    if !is_cocoa(window.api) {
        return false;
    }
    let parent = unsafe { window.specific.cocoa } as *mut AnyObject;
    if parent.is_null() {
        return false;
    }
    let Some(mut panel) = editor.pending.take() else {
        // Reparented: move the existing view.
        let Some(view) = &editor.view else { return false };
        unsafe {
            let _: () = msg_send![&**view, removeFromSuperview];
            let _: () = msg_send![parent, addSubview: &**view];
        }
        return true;
    };
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(editor.size.0, editor.size.1));
        let view: *mut AnyObject = msg_send![view_class(), alloc];
        let view: *mut AnyObject = msg_send![view, initWithFrame: frame];
        let Some(view) = Retained::from_raw(view as *mut NSView) else { return false };
        panel.attach(Retained::as_ptr(&view) as *mut c_void);
        let Some(ivar) = view.class().instance_variable(STATE_IVAR) else { return false };
        let state = Box::new(ViewState { panel, shared, gestures: BTreeSet::new() });
        *ivar.load_ptr::<*mut c_void>(&view) = Box::into_raw(state) as *mut c_void;
        let _: () = msg_send![parent, addSubview: &*view];
        let _: () = msg_send![&*view, updateTrackingAreas];

        // Common modes: frames keep coming while the host runs a modal
        // tracking loop (a knob drag in its own UI, a menu).
        let (Some(timer_class), Some(run_loop_class)) = (AnyClass::get(c"NSTimer"), AnyClass::get(c"NSRunLoop")) else {
            return false;
        };
        let nil: *mut AnyObject = std::ptr::null_mut();
        let timer: *mut AnyObject = msg_send![
            timer_class,
            timerWithTimeInterval: 1.0 / FRAME_RATE,
            target: &*view,
            selector: sel!(eseqTick:),
            userInfo: nil,
            repeats: Bool::YES
        ];
        let run_loop: *mut AnyObject = msg_send![run_loop_class, mainRunLoop];
        let common_modes = objc2_foundation::NSString::from_str("kCFRunLoopCommonModes");
        let _: () = msg_send![run_loop, addTimer: timer, forMode: &*common_modes];
        editor.timer = Retained::retain(timer);
        editor.view = Some(view);
        if let Some(state) = editor.state() {
            state.panel.frame();
        }
    }
    true
}

unsafe extern "C" fn gui_set_transient(_: *const clap_plugin, _: *const clap_window) -> bool {
    false
}

unsafe extern "C" fn gui_suggest_title(_: *const clap_plugin, _: *const c_char) {}

unsafe extern "C" fn gui_show(plugin: *const clap_plugin) -> bool {
    set_hidden(plugin, false)
}

unsafe extern "C" fn gui_hide(plugin: *const clap_plugin) -> bool {
    set_hidden(plugin, true)
}

fn set_hidden(plugin: *const clap_plugin, hidden: bool) -> bool {
    let Ok(slot) = unsafe { crate::shell::editor_slot(plugin) }.try_borrow() else { return false };
    let Some(editor) = slot.as_ref() else { return false };
    if let Some(view) = &editor.view {
        unsafe {
            let _: () = msg_send![&**view, setHidden: Bool::new(hidden)];
        }
    }
    true
}
