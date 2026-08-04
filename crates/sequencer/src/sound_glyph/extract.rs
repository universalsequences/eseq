//! Skeleton extraction per docs/sound-glyph-spec.md §1: cluster params by
//! longest shared snake_case prefix, assign top-level defs to clusters via
//! the def-reference graph (transitive param feeds), collapse linear def
//! chains into branch children, merge smallest clusters above the branch
//! cap. Everything here is deterministic: no HashMap iteration reaches the
//! output — ordering always comes from source order or explicit sorts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::geometry::fnv1a;
use super::sexpr::{parse, Sexpr};

/// Rendering cap from the spec (~30): above this, smallest clusters merge.
pub const MAX_BRANCHES: usize = 30;
/// Cluster that absorbs singleton params (no shared prefix with ≥2 members).
pub const GLOBAL_CLUSTER: &str = "global";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Cluster name (param prefix, `global`, or `a+b` for merged clusters).
    pub cluster: String,
    /// Visual heft: number of params + defs owned by the cluster.
    pub weight: usize,
    /// Collapsed def chains owned by this cluster (weight = chain length).
    pub children: Vec<Branch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skeleton {
    pub branches: Vec<Branch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedSkeleton {
    pub skeleton: Skeleton,
    /// param name → cluster name of the branch it belongs to.
    pub param_branch: BTreeMap<String, String>,
}

// ── source model ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeKind {
    Def,
    Defun,
    /// Traversal-only: macros resolve references but carry no weight.
    Macro,
    History,
}

struct Node {
    name: String,
    kind: NodeKind,
    src_idx: usize,
    refs: Vec<String>,
}

/// Symbols start with a letter or underscore; numeric literals start with a
/// digit, sign, or dot, so this check never drops symbols that merely *parse*
/// as floats (defs named `inf` / `infinity` / `nan` stay in the graph).
fn is_symbol(atom: &str) -> bool {
    let Some(first) = atom.chars().next() else {
        return false;
    };
    first.is_ascii_alphabetic() || first == '_'
}

/// Matches the reader's depth cap: the collectors stop descending here so
/// their recursion stays bounded even if a caller hands them a deep tree.
const MAX_COLLECT_DEPTH: usize = 256;

fn collect_symbols(form: &Sexpr, out: &mut Vec<String>, depth: usize) {
    match form {
        Sexpr::Atom(a) => {
            if is_symbol(a) {
                out.push(a.clone());
            }
        }
        Sexpr::List(items) => {
            if depth >= MAX_COLLECT_DEPTH {
                return;
            }
            for item in items {
                collect_symbols(item, out, depth + 1);
            }
        }
    }
}

/// Names bound by `def` / `make-history` forms anywhere inside `form`
/// (macro bodies declare locals that must not resolve to top-level nodes).
fn collect_local_names(form: &Sexpr, out: &mut Vec<String>, depth: usize) {
    if let Sexpr::List(items) = form {
        if let Some(head) = items.first().and_then(Sexpr::atom) {
            if matches!(head, "def" | "make-history") {
                if let Some(name) = items.get(1).and_then(Sexpr::atom) {
                    out.push(name.to_string());
                }
            }
        }
        if depth >= MAX_COLLECT_DEPTH {
            return;
        }
        for item in items {
            collect_local_names(item, out, depth + 1);
        }
    }
}

struct ParsedSource {
    /// Param names in declaration order.
    params: Vec<String>,
    nodes: Vec<Node>,
    node_index: HashMap<String, usize>,
    /// Whether the source parsed to any forms at all (comment/whitespace-only
    /// sources have none and legitimately yield an empty skeleton).
    has_forms: bool,
}

fn parse_source(source: &str) -> ParsedSource {
    let forms = parse(source);
    let mut params: Vec<String> = Vec::new();
    let mut param_set: HashSet<String> = HashSet::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut node_index: HashMap<String, usize> = HashMap::new();

    let push_node = |nodes: &mut Vec<Node>,
                     node_index: &mut HashMap<String, usize>,
                     name: &str,
                     kind: NodeKind,
                     src_idx: usize,
                     refs: Vec<String>| {
        if let Some(&existing) = node_index.get(name) {
            nodes[existing].refs.extend(refs);
        } else {
            node_index.insert(name.to_string(), nodes.len());
            nodes.push(Node {
                name: name.to_string(),
                kind,
                src_idx,
                refs,
            });
        }
    };

    for (src_idx, form) in forms.iter().enumerate() {
        let Sexpr::List(items) = form else { continue };
        let Some(head) = items.first().and_then(Sexpr::atom) else {
            continue;
        };
        match head {
            "param" => {
                if let Some(name) = items.get(1).and_then(Sexpr::atom) {
                    if param_set.insert(name.to_string()) {
                        params.push(name.to_string());
                    }
                }
            }
            "def" => {
                if let Some(name) = items.get(1).and_then(Sexpr::atom) {
                    let mut refs = Vec::new();
                    for item in &items[2..] {
                        collect_symbols(item, &mut refs, 0);
                    }
                    push_node(
                        &mut nodes,
                        &mut node_index,
                        name,
                        NodeKind::Def,
                        src_idx,
                        refs,
                    );
                }
            }
            "defun" | "defmacro" => {
                let Some(name) = items.get(1).and_then(Sexpr::atom) else {
                    continue;
                };
                let mut locals: Vec<String> = vec![name.to_string()];
                if let Some(Sexpr::List(args)) = items.get(2) {
                    for arg in args {
                        if let Some(a) = arg.atom() {
                            locals.push(a.to_string());
                        }
                    }
                }
                let mut refs = Vec::new();
                for item in items.iter().skip(3) {
                    collect_symbols(item, &mut refs, 0);
                    collect_local_names(item, &mut locals, 0);
                }
                let local_set: HashSet<&str> = locals.iter().map(String::as_str).collect();
                refs.retain(|r| !local_set.contains(r.as_str()));
                let kind = if head == "defun" {
                    NodeKind::Defun
                } else {
                    NodeKind::Macro
                };
                push_node(&mut nodes, &mut node_index, name, kind, src_idx, refs);
            }
            "make-history" => {
                if let Some(name) = items.get(1).and_then(Sexpr::atom) {
                    push_node(
                        &mut nodes,
                        &mut node_index,
                        name,
                        NodeKind::History,
                        src_idx,
                        Vec::new(),
                    );
                }
            }
            "write-history" => {
                // Feedback edge: whatever feeds the written value feeds the
                // history node read elsewhere.
                if let Some(name) = items.get(1).and_then(Sexpr::atom) {
                    let mut refs = Vec::new();
                    for item in &items[2..] {
                        collect_symbols(item, &mut refs, 0);
                    }
                    if let Some(&idx) = node_index.get(name) {
                        nodes[idx].refs.extend(refs);
                    }
                }
            }
            _ => {}
        }
    }

    ParsedSource {
        params,
        nodes,
        node_index,
        has_forms: !forms.is_empty(),
    }
}

// ── param clustering ──

fn snake_prefixes(name: &str) -> Vec<String> {
    // Empty segments (leading/double underscores) are dropped so `_a`-style
    // params never cluster under an empty-string prefix.
    let segments: Vec<&str> = name.split('_').filter(|s| !s.is_empty()).collect();
    (1..=segments.len())
        .map(|n| segments[..n].join("_"))
        .collect()
}

pub(super) struct ParamCluster {
    pub name: String,
    /// Indices into the param declaration order (source order).
    pub members: Vec<usize>,
}

/// Longest shared snake_case prefix with ≥2 members; singletons fold into
/// `global`. A sub-prefix cluster folds into its parent cluster when one
/// exists (`lfo_to_*` joins `lfo`); sub-groups only stand alone when no
/// parent cluster formed. Cluster order = source order of each cluster's
/// first param.
pub(super) fn cluster_params(params: &[String]) -> Vec<ParamCluster> {
    let mut prefix_counts: HashMap<String, usize> = HashMap::new();
    for param in params {
        for prefix in snake_prefixes(param) {
            *prefix_counts.entry(prefix).or_insert(0) += 1;
        }
    }
    let mut clusters: Vec<ParamCluster> = Vec::new();
    let mut cluster_index: HashMap<String, usize> = HashMap::new();
    for (idx, param) in params.iter().enumerate() {
        let cluster_name = snake_prefixes(param)
            .into_iter()
            .rev()
            .find(|p| prefix_counts.get(p).copied().unwrap_or(0) >= 2)
            .unwrap_or_else(|| GLOBAL_CLUSTER.to_string());
        if let Some(&existing) = cluster_index.get(&cluster_name) {
            clusters[existing].members.push(idx);
        } else {
            cluster_index.insert(cluster_name.clone(), clusters.len());
            clusters.push(ParamCluster {
                name: cluster_name,
                members: vec![idx],
            });
        }
    }

    // Fold sub-prefix clusters into their nearest parent cluster.
    loop {
        let mut merge: Option<(usize, usize)> = None;
        'search: for (child_idx, child) in clusters.iter().enumerate() {
            if child.name == GLOBAL_CLUSTER {
                continue;
            }
            // Strict prefixes of the cluster name, longest first.
            for prefix in snake_prefixes(&child.name).iter().rev().skip(1) {
                if let Some(parent_idx) = clusters.iter().position(|c| &c.name == prefix) {
                    merge = Some((child_idx, parent_idx));
                    break 'search;
                }
            }
        }
        let Some((child_idx, parent_idx)) = merge else {
            break;
        };
        let members = clusters.remove(child_idx).members;
        let parent_idx = if parent_idx > child_idx {
            parent_idx - 1
        } else {
            parent_idx
        };
        clusters[parent_idx].members.extend(members);
        clusters[parent_idx].members.sort_unstable();
    }
    clusters.sort_by_key(|c| c.members[0]);
    clusters
}

// ── branch assembly (shared with the stock-skeleton path) ──

pub(super) struct ProtoBranch {
    /// Ordering key: source index of the cluster's first param.
    pub pos: usize,
    pub name: String,
    pub weight: usize,
    pub children: Vec<Branch>,
    /// Param names belonging to this branch (feeds `param_branch`).
    pub params: Vec<String>,
}

/// Order by first-param source position, then merge smallest-first (weight,
/// then name) until at most `MAX_BRANCHES` remain. Merged branches keep the
/// earlier position and join names with `+`.
pub(super) fn finalize_branches(mut protos: Vec<ProtoBranch>) -> ExtractedSkeleton {
    while protos.len() > MAX_BRANCHES {
        let mut order: Vec<usize> = (0..protos.len()).collect();
        order.sort_by(|&a, &b| {
            (protos[a].weight, protos[a].name.as_str())
                .cmp(&(protos[b].weight, protos[b].name.as_str()))
        });
        let (first, second) = (order[0], order[1]);
        let (keep, gone) = (first.min(second), first.max(second));
        let absorbed = protos.remove(gone);
        let survivor = &mut protos[keep];
        // Name order follows the merge pick order (smaller first).
        let (a, b) = if keep == first {
            (survivor.name.clone(), absorbed.name)
        } else {
            (absorbed.name, survivor.name.clone())
        };
        survivor.name = format!("{a}+{b}");
        survivor.pos = survivor.pos.min(absorbed.pos);
        survivor.weight += absorbed.weight;
        survivor.children.extend(absorbed.children);
        survivor.params.extend(absorbed.params);
    }
    protos.sort_by(|a, b| a.pos.cmp(&b.pos).then_with(|| a.name.cmp(&b.name)));

    let mut param_branch = BTreeMap::new();
    let mut branches = Vec::with_capacity(protos.len());
    for proto in protos {
        for param in &proto.params {
            param_branch.insert(param.clone(), proto.name.clone());
        }
        branches.push(Branch {
            cluster: proto.name,
            weight: proto.weight,
            children: proto.children,
        });
    }
    ExtractedSkeleton {
        skeleton: Skeleton { branches },
        param_branch,
    }
}

// ── extraction ──

/// Extract the glyph skeleton from an instrument's authored dgenlisp source.
pub fn extract_skeleton(source: &str) -> ExtractedSkeleton {
    let parsed = parse_source(source);
    if parsed.params.is_empty() {
        return param_less_skeleton(source, &parsed);
    }
    let clusters = cluster_params(&parsed.params);

    let mut param_cluster: HashMap<&str, usize> = HashMap::new();
    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        for &param_idx in &cluster.members {
            param_cluster.insert(parsed.params[param_idx].as_str(), cluster_idx);
        }
    }

    // Fixpoint over the reference graph: feed[i] = clusters whose params
    // transitively feed node i. Handles feedback cycles (write-history).
    let n = parsed.nodes.len();
    let mut feed: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for (i, node) in parsed.nodes.iter().enumerate() {
        for r in &node.refs {
            if let Some(&c) = param_cluster.get(r.as_str()) {
                feed[i].insert(c);
            }
        }
    }
    loop {
        let mut changed = false;
        for i in 0..n {
            let mut incoming: BTreeSet<usize> = BTreeSet::new();
            for r in &parsed.nodes[i].refs {
                if param_cluster.contains_key(r.as_str()) {
                    continue;
                }
                if let Some(&j) = parsed.node_index.get(r) {
                    incoming.extend(feed[j].iter().copied());
                }
            }
            let before = feed[i].len();
            feed[i].extend(incoming);
            if feed[i].len() != before {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Effective node→node references, seen through macros (a def that calls
    // a macro references whatever the macro body references).
    let effective_refs: Vec<BTreeSet<usize>> = (0..n)
        .map(|i| {
            let mut out = BTreeSet::new();
            let mut stack: Vec<&str> = parsed.nodes[i].refs.iter().map(String::as_str).collect();
            let mut seen: HashSet<&str> = HashSet::new();
            while let Some(r) = stack.pop() {
                if param_cluster.contains_key(r) || !seen.insert(r) {
                    continue;
                }
                if let Some(&j) = parsed.node_index.get(r) {
                    if parsed.nodes[j].kind == NodeKind::Macro {
                        stack.extend(parsed.nodes[j].refs.iter().map(String::as_str));
                    } else if j != i {
                        out.insert(j);
                    }
                }
            }
            out
        })
        .collect();

    // Owned defs: weight-bearing nodes fed by exactly one cluster. Nodes fed
    // by several clusters (or none) are trunk material, not branch heft.
    let mut owned_by_cluster: Vec<Vec<usize>> = vec![Vec::new(); clusters.len()];
    for (i, node) in parsed.nodes.iter().enumerate() {
        if node.kind == NodeKind::Macro {
            continue;
        }
        if feed[i].len() == 1 {
            let c = *feed[i].iter().next().unwrap();
            owned_by_cluster[c].push(i);
        }
    }

    let mut protos = Vec::with_capacity(clusters.len());
    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        let owned = &owned_by_cluster[cluster_idx];
        let children = collapse_chains(&parsed.nodes, &effective_refs, owned);
        protos.push(ProtoBranch {
            pos: cluster.members[0],
            name: cluster.name.clone(),
            weight: cluster.members.len() + owned.len(),
            children,
            params: cluster
                .members
                .iter()
                .map(|&i| parsed.params[i].clone())
                .collect(),
        });
    }
    finalize_branches(protos)
}

/// Never-empty invariant (sound-glyph + delta-glyph specs): every non-empty
/// source gets a stable, non-empty skeleton even without `(param …)` forms.
/// Weight-bearing top-level defs become the branches; a source with forms
/// but no defs collapses to a single trunk branch whose name is seeded by
/// the FNV-1a hash of the source, so distinct sources keep distinct glyph
/// identities. Comment/whitespace-only sources still yield an empty skeleton.
fn param_less_skeleton(source: &str, parsed: &ParsedSource) -> ExtractedSkeleton {
    let protos: Vec<ProtoBranch> = parsed
        .nodes
        .iter()
        .filter(|node| node.kind != NodeKind::Macro)
        .map(|node| ProtoBranch {
            pos: node.src_idx,
            name: node.name.clone(),
            weight: 1,
            children: Vec::new(),
            params: Vec::new(),
        })
        .collect();
    if !protos.is_empty() {
        return finalize_branches(protos);
    }
    if !parsed.has_forms {
        return ExtractedSkeleton::default();
    }
    finalize_branches(vec![ProtoBranch {
        pos: 0,
        name: format!("src_{:016x}", fnv1a(source.as_bytes())),
        weight: 1,
        children: Vec::new(),
        params: Vec::new(),
    }])
}

/// Collapse linear chains in a cluster's owned-def subgraph: a producer with
/// exactly one owned consumer merges into that consumer when it is the
/// consumer's only owned producer. Each collapsed group becomes one child
/// branch named after its source-latest def, weighted by chain length,
/// ordered by its source-earliest def.
fn collapse_chains(
    nodes: &[Node],
    effective_refs: &[BTreeSet<usize>],
    owned: &[usize],
) -> Vec<Branch> {
    let owned_set: HashSet<usize> = owned.iter().copied().collect();
    let producers: HashMap<usize, Vec<usize>> = owned
        .iter()
        .map(|&i| {
            let mut p: Vec<usize> = effective_refs[i]
                .iter()
                .copied()
                .filter(|j| owned_set.contains(j))
                .collect();
            p.sort_unstable();
            (i, p)
        })
        .collect();
    let mut consumers: HashMap<usize, Vec<usize>> =
        owned.iter().map(|&i| (i, Vec::new())).collect();
    for &i in owned {
        for &j in &producers[&i] {
            consumers.get_mut(&j).unwrap().push(i);
        }
    }

    // Union-find over owned nodes.
    let mut parent: HashMap<usize, usize> = owned.iter().map(|&i| (i, i)).collect();
    fn find(parent: &mut HashMap<usize, usize>, mut x: usize) -> usize {
        while parent[&x] != x {
            let grand = parent[&parent[&x]];
            parent.insert(x, grand);
            x = grand;
        }
        x
    }
    for &b in owned {
        let cons = &consumers[&b];
        if cons.len() != 1 {
            continue;
        }
        let a = cons[0];
        if producers[&a].len() == 1 {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in owned {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }

    let mut children: Vec<(usize, Branch)> = groups
        .into_values()
        .map(|mut members| {
            members.sort_by_key(|&i| nodes[i].src_idx);
            let first = nodes[members[0]].src_idx;
            let tip = *members.last().unwrap();
            (
                first,
                Branch {
                    cluster: nodes[tip].name.clone(),
                    weight: members.len(),
                    children: Vec::new(),
                },
            )
        })
        .collect();
    children.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cluster.cmp(&b.1.cluster)));
    children.into_iter().map(|(_, b)| b).collect()
}
