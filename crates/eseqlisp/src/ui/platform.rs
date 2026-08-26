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

pub fn is_exact_primary_shortcut_modifier_for(
    modifiers: KeyModifiers,
    platform: ShortcutPlatform,
) -> bool {
    modifiers == primary_shortcut_modifier_for(platform)
}

pub fn is_exact_primary_shortcut_modifier(modifiers: KeyModifiers) -> bool {
    is_exact_primary_shortcut_modifier_for(modifiers, CURRENT_SHORTCUT_PLATFORM)
}

/// Exact modifier sets accepted by editor keymaps for copy/paste.
///
/// Sequencer shortcuts test for containment, so Shift naturally remains an
/// optional modifier. Editor bindings are exact and therefore need both Linux
/// forms registered explicitly. macOS intentionally retains its existing
/// Cmd-only bindings.
pub fn primary_clipboard_key_modifiers() -> impl Iterator<Item = KeyModifiers> {
    let primary = primary_shortcut_modifier();
    [
        Some(primary),
        (CURRENT_SHORTCUT_PLATFORM != ShortcutPlatform::MacOS)
            .then_some(primary | KeyModifiers::SHIFT),
    ]
    .into_iter()
    .flatten()
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
