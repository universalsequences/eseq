/// Shared application grid font size. Backends must agree because widget
/// dimensions and tile constraints are authored in units of this cell grid.
pub(crate) const DEFAULT_MONOSPACE_FONT_SIZE_PT: f64 = 16.0;

/// Live-editable shader overrides read from `env!("CARGO_MANIFEST_DIR")` in
/// this crate's source tree. That path only exists in a checkout, so a
/// packaged application must switch the watch off: otherwise every rendered
/// frame pays an `fs::metadata` on a path that can never resolve. Hosts call
/// [`set_editable_shader_overrides_enabled`] once at startup; the default
/// stays on so `cargo run` and the tests are unaffected.
static EDITABLE_SHADER_OVERRIDES_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_editable_shader_overrides_enabled(enabled: bool) {
    EDITABLE_SHADER_OVERRIDES_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn editable_shader_overrides_enabled() -> bool {
    EDITABLE_SHADER_OVERRIDES_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub mod backend;
pub mod drag_profile;
pub mod frame;
pub(crate) mod gpu_geometry;
pub(crate) mod gpu_scene;
pub mod glyph_atlas;
pub mod hit;
pub mod layout;
pub mod metal_backend;
pub mod platform;
pub(crate) mod pointer_input;
pub mod theme;
pub mod tui;
pub mod wgsl_shaders;
#[cfg(feature = "wgpu")]
pub mod gpu_adapter;
#[cfg(feature = "wgpu")]
pub mod wgpu_app;
#[cfg(feature = "wgpu")]
pub mod wgpu_backend;
#[cfg(feature = "wgpu")]
pub mod wgpu_frame_stats;
#[cfg(feature = "wgpu")]
pub mod wgpu_pipelines;
