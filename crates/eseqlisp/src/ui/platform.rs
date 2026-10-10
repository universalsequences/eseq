//! Small platform integrations shared by graphical render backends.
//!
//! Rendering backends should use these helpers rather than exposing AppKit
//! types to otherwise portable UI code.

use crossterm::event::KeyModifiers;
use winit::window::{Theme as WindowTheme, Window};

use crate::backend::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutPlatform {
    MacOS,
    Other,
}

pub const CURRENT_SHORTCUT_PLATFORM: ShortcutPlatform = if cfg!(target_os = "macos") {
    ShortcutPlatform::MacOS
} else {
    ShortcutPlatform::Other
};

pub const fn primary_shortcut_modifier_for(platform: ShortcutPlatform) -> KeyModifiers {
    match platform {
        ShortcutPlatform::MacOS => KeyModifiers::SUPER,
        ShortcutPlatform::Other => KeyModifiers::CONTROL,
    }
}

pub const fn primary_shortcut_modifier() -> KeyModifiers {
    primary_shortcut_modifier_for(CURRENT_SHORTCUT_PLATFORM)
}

pub fn has_primary_shortcut_modifier_for(
    modifiers: KeyModifiers,
    platform: ShortcutPlatform,
) -> bool {
    modifiers.contains(primary_shortcut_modifier_for(platform))
}

pub fn has_primary_shortcut_modifier(modifiers: KeyModifiers) -> bool {
    has_primary_shortcut_modifier_for(modifiers, CURRENT_SHORTCUT_PLATFORM)
}

/// Modifier test for global sequencer shortcuts (track grouping, mods view,
/// pattern length, select-all).
///
/// These bindings historically accepted either Ctrl or Cmd on macOS, so the
/// Ctrl aliases stay alive there. Linux and other targets keep Ctrl as the sole
/// primary chord; Super remains the window manager's key.
pub fn has_sequencer_shortcut_modifier_for(
    modifiers: KeyModifiers,
    platform: ShortcutPlatform,
) -> bool {
    has_primary_shortcut_modifier_for(modifiers, platform)
        || (platform == ShortcutPlatform::MacOS && modifiers.contains(KeyModifiers::CONTROL))
}

pub fn has_sequencer_shortcut_modifier(modifiers: KeyModifiers) -> bool {
    has_sequencer_shortcut_modifier_for(modifiers, CURRENT_SHORTCUT_PLATFORM)
}

pub fn is_exact_primary_shortcut_modifier_for(
    modifiers: KeyModifiers,
    platform: ShortcutPlatform,
) -> bool {
    modifiers == primary_shortcut_modifier_for(platform)
}

pub fn is_exact_primary_shortcut_modifier(modifiers: KeyModifiers) -> bool {
    is_exact_primary_shortcut_modifier_for(modifiers, CURRENT_SHORTCUT_PLATFORM)
}

/// Exact modifier accepted by editor keymaps for copy/paste.
///
/// macOS uses Cmd-C/V. Linux and other targets use Ctrl-Shift-C/V, preserving
/// Ctrl-C as the prefix of conventional editor chords such as C-c C-c.
pub const fn clipboard_shortcut_modifier_for(platform: ShortcutPlatform) -> KeyModifiers {
    match platform {
        ShortcutPlatform::MacOS => KeyModifiers::SUPER,
        ShortcutPlatform::Other => KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
    }
}

pub fn primary_clipboard_key_modifiers() -> impl Iterator<Item = KeyModifiers> {
    [clipboard_shortcut_modifier_for(CURRENT_SHORTCUT_PLATFORM)].into_iter()
}

pub fn window_theme_for_background(background: Color) -> WindowTheme {
    if background.luma() > 0.55 {
        WindowTheme::Light
    } else {
        WindowTheme::Dark
    }
}

/// Keep native window chrome consistent with eseqlisp's active theme.
///
/// On Linux and other non-macOS targets winit maps this to the platform window
/// system. macOS additionally needs AppKit calls for its transparent titlebar
/// and exact background color.
pub fn sync_window_theme(window: &Window, background: Color) {
    let window_theme = window_theme_for_background(background);
    window.set_theme(Some(window_theme));

    #[cfg(target_os = "macos")]
    sync_appkit_window_theme(window, background, window_theme);
}

#[cfg(target_os = "macos")]
fn sync_appkit_window_theme(window: &Window, background: Color, window_theme: WindowTheme) {
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSColor, NSView};
    use objc2_foundation::NSString;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };

    unsafe {
        let ns_view = appkit.ns_view.as_ptr() as *mut NSView;
        let ns_view = &*ns_view;
        let Some(ns_window) = ns_view.window() else {
            return;
        };
        let color = NSColor::colorWithRed_green_blue_alpha(
            background.r as f64,
            background.g as f64,
            background.b as f64,
            1.0,
        );
        ns_window.setBackgroundColor(Some(&color));
        ns_window.setTitlebarAppearsTransparent(true);

        let appearance_name = match window_theme {
            WindowTheme::Light => "NSAppearanceNameVibrantLight",
            WindowTheme::Dark => "NSAppearanceNameVibrantDark",
        };
        if let Some(appearance) =
            NSAppearance::appearanceNamed(&NSString::from_str(appearance_name))
        {
            ns_window.setAppearance(Some(&appearance));
        }
    }
}

#[cfg(target_os = "macos")]
fn appkit_window(window: &Window) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let ns_view = unsafe { &*(appkit.ns_view.as_ptr() as *mut NSView) };
    ns_view.window()
}

/// The native titlebar band drawn over a full-size content view, in logical
/// points: its height, and the width its window buttons occupy at the left
/// end (zero in full screen, where AppKit hides the band).
#[cfg(target_os = "macos")]
pub fn native_titlebar_metrics(window: &Window) -> (f64, f64) {
    use objc2_app_kit::NSWindowButton;

    let Some(ns_window) = appkit_window(window) else {
        return (0.0, 0.0);
    };
    let frame = ns_window.frame();
    let band = (frame.size.height - ns_window.contentLayoutRect().size.height).max(0.0);
    if band <= 0.0 {
        return (0.0, 0.0);
    }
    // The zoom button is the rightmost traffic light; leave the same gap
    // after it that AppKit leaves before the close button.
    let leading = ns_window
        .standardWindowButton(NSWindowButton::ZoomButton)
        .zip(ns_window.standardWindowButton(NSWindowButton::CloseButton))
        .filter(|(zoom, _)| !zoom.isHidden())
        .map(|(zoom, close)| {
            let zoom = zoom.convertRect_toView(zoom.bounds(), None);
            let close = close.convertRect_toView(close.bounds(), None);
            zoom.origin.x + zoom.size.width + close.origin.x
        })
        .unwrap_or(0.0);
    (band, leading)
}

#[cfg(not(target_os = "macos"))]
pub fn native_titlebar_metrics(_window: &Window) -> (f64, f64) {
    (0.0, 0.0)
}

/// Stop AppKit from dragging the window by its titlebar band on its own.
/// The content view sits under the band, and AppKit would otherwise turn any
/// drag that starts there into a window move — including drags of the
/// widgets drawn in it. Window moves then come only from
/// [`titlebar_mouse_down`] on empty band space (`performWindowDragWithEvent:`
/// still works on a window that is not movable).
#[cfg(target_os = "macos")]
pub fn disable_native_window_drag(window: &Window) {
    if let Some(ns_window) = appkit_window(window) {
        ns_window.setMovable(false);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn disable_native_window_drag(_window: &Window) {}

/// A press on an empty part of the titlebar band: drag the window, or on a
/// double-click (`click_count` >= 2) do what System Settings > Desktop & Dock
/// asks for (zoom by default).
///
/// winit hands the press over from a run-loop observer, after AppKit's
/// current event has moved on, so the drag gets a mouse-down event made here
/// rather than `currentEvent` (an AppKit call on a non-mouse event raises an
/// Objective-C exception, which aborts a Rust process). The AppKit call is
/// also deferred to the next run-loop pass with `performSelector:afterDelay:`:
/// dragging and zooming run tracking loops that dispatch events back into
/// winit, and doing that from inside winit's own callback re-enters it.
#[cfg(target_os = "macos")]
pub fn titlebar_mouse_down(window: &Window, click_count: usize) {
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{msg_send, sel};
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSProcessInfo, NSString, NSUserDefaults};

    let Some(ns_window) = appkit_window(window) else {
        return;
    };
    let defer = |selector: Sel, argument: Option<&AnyObject>| unsafe {
        let _: () = msg_send![
            &*ns_window,
            performSelector: selector,
            withObject: argument,
            afterDelay: 0.0f64
        ];
    };
    if click_count >= 2 {
        let action = NSUserDefaults::standardUserDefaults()
            .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
            .map(|action| action.to_string());
        match action.as_deref() {
            Some("None") => {}
            Some("Minimize") => defer(sel!(miniaturize:), None),
            _ => defer(sel!(zoom:), None),
        }
        return;
    }
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDown,
        ns_window.mouseLocationOutsideOfEventStream(),
        NSEventModifierFlags::empty(),
        NSProcessInfo::processInfo().systemUptime(),
        ns_window.windowNumber(),
        None,
        0,
        1,
        1.0,
    );
    if let Some(event) = event {
        // The window is not movable (see `disable_native_window_drag`), and
        // AppKit honours that for this drag too: allow it for just this one.
        // `performWindowDragWithEvent:` tracks the drag before returning.
        ns_window.setMovable(true);
        defer(sel!(performWindowDragWithEvent:), Some(&event));
        // A nil object is a zero BOOL: `setMovable:NO`, queued behind the drag.
        defer(sel!(setMovable:), None);
    }
}

/// The user's double-click interval (System Settings > Mouse).
#[cfg(target_os = "macos")]
pub fn double_click_interval() -> std::time::Duration {
    std::time::Duration::from_secs_f64(objc2_app_kit::NSEvent::doubleClickInterval().clamp(0.1, 2.0))
}

#[cfg(not(target_os = "macos"))]
pub fn double_click_interval() -> std::time::Duration {
    std::time::Duration::from_millis(500)
}

#[cfg(not(target_os = "macos"))]
pub fn titlebar_mouse_down(window: &Window, _click_count: usize) {
    let _ = window.drag_window();
}

#[cfg(target_os = "macos")]
pub fn trigger_level_change_haptic() {
    use objc2_app_kit::{
        NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
        NSHapticFeedbackPerformer,
    };

    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(
        NSHapticFeedbackPattern::LevelChange,
        NSHapticFeedbackPerformanceTime::Now,
    );
}

#[cfg(not(target_os = "macos"))]
pub fn trigger_level_change_haptic() {}

#[cfg(target_os = "macos")]
pub fn trigger_alignment_haptic() {
    use objc2_app_kit::{
        NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
        NSHapticFeedbackPerformer,
    };

    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(
        NSHapticFeedbackPattern::Alignment,
        NSHapticFeedbackPerformanceTime::Now,
    );
}

#[cfg(not(target_os = "macos"))]
pub fn trigger_alignment_haptic() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_shortcuts_keep_the_macos_control_alias() {
        for modifiers in [KeyModifiers::SUPER, KeyModifiers::CONTROL] {
            assert!(has_sequencer_shortcut_modifier_for(
                modifiers,
                ShortcutPlatform::MacOS
            ));
        }
        assert!(has_sequencer_shortcut_modifier_for(
            KeyModifiers::CONTROL,
            ShortcutPlatform::Other
        ));
        assert!(!has_sequencer_shortcut_modifier_for(
            KeyModifiers::SUPER,
            ShortcutPlatform::Other
        ));
        assert!(!has_sequencer_shortcut_modifier_for(
            KeyModifiers::ALT,
            ShortcutPlatform::MacOS
        ));
    }

    #[test]
    fn clipboard_shortcuts_preserve_control_chords_off_macos() {
        assert_eq!(
            clipboard_shortcut_modifier_for(ShortcutPlatform::MacOS),
            KeyModifiers::SUPER
        );
        assert_eq!(
            clipboard_shortcut_modifier_for(ShortcutPlatform::Other),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        );
    }

    #[test]
    fn window_theme_follows_ui_background_luma() {
        assert_eq!(
            window_theme_for_background(Color::rgb(0.9, 0.9, 0.9)),
            WindowTheme::Light
        );
        assert_eq!(
            window_theme_for_background(Color::rgb(0.1, 0.1, 0.1)),
            WindowTheme::Dark
        );
    }
}
