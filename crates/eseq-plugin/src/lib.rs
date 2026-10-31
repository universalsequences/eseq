//! The eseq plugin shell: one prebuilt binary that becomes any exported eseq
//! instrument. It reads its identity, params, DSP library and panel from the
//! payload inside its own bundle (`Contents/Resources/eseq-payload`), speaks
//! CLAP directly, and gets VST3 from clap-wrapper wrapping that CLAP entry.
//!
//! See `docs/plugin-export-spec.md`.

pub mod engine;
pub use eseq_plugin_format as payload;
pub mod shell;

#[cfg(target_os = "macos")]
pub mod editor;
#[cfg(target_os = "macos")]
mod app_natives;
#[cfg(target_os = "macos")]
pub mod panel;

pub use shell::clap_entry;

clap_wrapper::export_vst3!();
