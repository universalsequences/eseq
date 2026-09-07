//! Voice allocation and lifecycle management for the real-time audio engine.
//!
//! `pool` contains the compact allocator used by sampler and graph-node voices.
//! `runtime` adds custom-engine routing, release tails, and topology sync.

mod mono;
mod pool;
mod runtime;

pub(super) use mono::{MonoHeldNotes, MonoRelease};
pub(super) use pool::VoicePool;
pub use pool::MAX_VOICES;
pub(super) use runtime::*;
