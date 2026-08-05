//! Stock skeletons for builtins (sampler + builtin effects), which have no
//! lisp source. v1 is a generic radial: branches come from the builtin's
//! param groups (`ParamUiMetadata.group` when authored, prefix clustering as
//! fallback), same `Skeleton` type so the geometry/diff layers work
//! unchanged. Hand-authored per-builtin character comes later.

use crate::effects::EffectDescriptor;

use super::extract::{cluster_params, finalize_branches, ExtractedSkeleton, ProtoBranch};

fn normalize_group(group: &str) -> String {
    group.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// Generic radial skeleton from a builtin's param groups. Branch order =
/// declaration order of each group's first param; weight = param count.
pub fn stock_skeleton(descriptor: &EffectDescriptor) -> ExtractedSkeleton {
    let mut protos: Vec<ProtoBranch> = Vec::new();
    let mut ungrouped: Vec<(usize, String)> = Vec::new();

    for (idx, param) in descriptor.params.iter().enumerate() {
        let group = param
            .ui_metadata
            .as_ref()
            .and_then(|meta| meta.group.as_deref())
            .map(normalize_group)
            .filter(|g| !g.is_empty());
        match group {
            Some(name) => {
                if let Some(proto) = protos.iter_mut().find(|p| p.name == name) {
                    proto.weight += 1;
                    proto.params.push(param.name.clone());
                } else {
                    protos.push(ProtoBranch {
                        pos: idx,
                        name,
                        weight: 1,
                        children: Vec::new(),
                        params: vec![param.name.clone()],
                    });
                }
            }
            None => ungrouped.push((idx, param.name.clone())),
        }
    }

    // Ungrouped params fall back to the same prefix clustering as authored
    // sources (singletons land in `global`).
    let names: Vec<String> = ungrouped.iter().map(|(_, name)| name.clone()).collect();
    for cluster in cluster_params(&names) {
        let params: Vec<String> = cluster
            .members
            .iter()
            .map(|&i| ungrouped[i].1.clone())
            .collect();
        let pos = ungrouped[cluster.members[0]].0;
        if let Some(proto) = protos.iter_mut().find(|p| p.name == cluster.name) {
            proto.pos = proto.pos.min(pos);
            proto.weight += params.len();
            proto.params.extend(params);
        } else {
            protos.push(ProtoBranch {
                pos,
                name: cluster.name,
                weight: params.len(),
                children: Vec::new(),
                params,
            });
        }
    }

    finalize_branches(protos)
}
