//! Sound glyph skeleton extraction (docs/sound-glyph-spec.md §1, phase P1).
//!
//! Turns an instrument's authored dgenlisp source (pre-expansion, never the
//! expanded DSP graph) into the topology skeleton that drives the plant
//! glyph: params clustered by longest shared snake_case prefix, top-level
//! defs assigned to clusters via the def-reference graph, linear def chains
//! collapsed into branch children. Pure — no UI, no rendering.
//!
//! Builtins (sampler + builtin effects) have no lisp source; they get a
//! generic radial skeleton derived from their `EffectDescriptor` param
//! groups via [`stock_skeleton`].

mod extract;
mod geometry;
mod sexpr;
mod stock;
#[cfg(test)]
mod tests;

pub use extract::{
    extract_skeleton, Branch, ExtractedSkeleton, Skeleton, GLOBAL_CLUSTER, MAX_BRANCHES,
};
pub use geometry::{
    param_ranges, param_specs, resolve_geometry, GlyphGeometry, GlyphMark, GlyphStroke, ParamSpec,
    TRUNK,
};
pub use stock::stock_skeleton;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Cache key for a skeleton: hash of the raw instrument source.
pub fn source_hash(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Memoizes skeleton extraction per instrument source hash; recompute only
/// happens when the instrument source changes.
#[derive(Default)]
pub struct SkeletonCache {
    entries: HashMap<u64, Arc<ExtractedSkeleton>>,
}

impl SkeletonCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_extract(&mut self, source: &str) -> Arc<ExtractedSkeleton> {
        let key = source_hash(source);
        Arc::clone(
            self.entries
                .entry(key)
                .or_insert_with(|| Arc::new(extract_skeleton(source))),
        )
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
