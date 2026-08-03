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

use crate::delta_glyph::IdentityBranch;

/// Flatten a skeleton into the delta glyph's identity tier (delta-glyph spec
/// §5.1a). Prefix clustering can collapse an instrument whose params share no
/// prefixes into a single `global` branch — one slot renders as one giant
/// disc — so thin skeletons expand into their collapsed def-chain children,
/// which carry the AST's actual dataflow structure at low resolution.
pub fn identity_branches(skeleton: &Skeleton) -> Vec<IdentityBranch> {
    const THIN: usize = 6;
    let direct = skeleton
        .branches
        .iter()
        .map(|branch| IdentityBranch {
            name: branch.cluster.clone(),
            weight: branch.weight.max(1) as f32,
        })
        .collect::<Vec<_>>();
    if direct.len() >= THIN {
        return direct;
    }
    let expanded = skeleton
        .branches
        .iter()
        .flat_map(|branch| {
            if branch.children.is_empty() {
                vec![IdentityBranch {
                    name: branch.cluster.clone(),
                    weight: branch.weight.max(1) as f32,
                }]
            } else {
                branch
                    .children
                    .iter()
                    .map(|child| IdentityBranch {
                        name: format!("{}/{}", branch.cluster, child.cluster),
                        weight: child.weight.max(1) as f32,
                    })
                    .collect()
            }
        })
        .collect::<Vec<_>>();
    if expanded.len() > direct.len() { expanded } else { direct }
}

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
