//! Small platform integrations shared by graphical render backends.
//!
//! Rendering backends should use these helpers rather than exposing AppKit
//! types to otherwise portable UI code.

use winit::window::{Theme as WindowTheme, Window};

use crate::backend::Color;

pub(crate) fn window_theme_for_background(background: Color) -> WindowTheme {
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
pub(crate) fn sync_window_theme(window: &Window, background: Color) {
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
pub(crate) fn trigger_level_change_haptic() {
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
pub(crate) fn trigger_level_change_haptic() {}

#[cfg(target_os = "macos")]
pub(crate) fn trigger_alignment_haptic() {
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
pub(crate) fn trigger_alignment_haptic() {}

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
