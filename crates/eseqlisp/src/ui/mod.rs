/// Shared application grid font size. Backends must agree because widget
/// dimensions and tile constraints are authored in units of this cell grid.
pub(crate) const DEFAULT_MONOSPACE_FONT_SIZE_PT: f64 = 16.0;

pub mod backend;
pub mod frame;
pub(crate) mod gpu_geometry;
pub(crate) mod gpu_scene;
pub mod glyph_atlas;
pub mod hit;
pub mod layout;
pub mod metal_backend;
pub mod platform;
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
