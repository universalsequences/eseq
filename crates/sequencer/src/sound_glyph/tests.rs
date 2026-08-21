use std::fs;

use crate::effects::EffectDescriptor;

use super::*;

fn instrument_source(rel: &str) -> String {
    let path = crate::app_paths::app_paths().instruments_dir().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn cluster_names(skeleton: &Skeleton) -> Vec<&str> {
    skeleton
        .branches
        .iter()
        .map(|b| b.cluster.as_str())
        .collect()
}

fn branch<'a>(extracted: &'a ExtractedSkeleton, cluster: &str) -> &'a Branch {
    extracted
        .skeleton
        .branches
        .iter()
        .find(|b| b.cluster == cluster)
        .unwrap_or_else(|| panic!("missing branch {cluster}"))
}

// ── operator ──

#[test]
fn operator_cluster_names_and_order() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    assert_eq!(
        cluster_names(&extracted.skeleton),
        vec![
            "opa", "opb", "opc", "opd", "partial", "global", "penv", "fenv", "env", "lfo",
            "filter", "shaper",
        ],
    );
}

#[test]
fn operator_param_membership() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let expect = [
        ("opa_attack", "opa"),
        ("opa_env_mode", "opa"),
        ("opd_level_db", "opd"),
        ("partial_7", "partial"),
        ("penv_amount", "penv"),
        ("fenv_amt", "fenv"),
        ("env_loop_rate_hz", "env"),
        ("env_sync_div", "env"),
        ("lfo_to_filter", "lfo"),
        ("filter_keytrack", "filter"),
        ("shaper_wet", "shaper"),
        ("tone", "global"),
        ("algorithm", "global"),
        ("fm_drive_db", "global"),
        ("feedback", "global"),
        ("user_norm", "global"),
        ("volume_db", "global"),
    ];
    for (param, cluster) in expect {
        assert_eq!(
            extracted.param_branch.get(param).map(String::as_str),
            Some(cluster),
            "param {param}",
        );
    }
    // Every operator param is mapped to an existing branch.
    assert_eq!(extracted.param_branch.len(), 110);
    for (param, cluster) in &extracted.param_branch {
        assert!(
            extracted
                .skeleton
                .branches
                .iter()
                .any(|b| &b.cluster == cluster),
            "param {param} maps to unknown branch {cluster}",
        );
    }
}

#[test]
fn operator_weights_cover_params_and_owned_defs() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let param_counts = [
        ("opa", 13),
        ("opb", 13),
        ("opc", 13),
        ("opd", 13),
        ("partial", 16),
        ("penv", 6),
        ("fenv", 6),
        ("env", 2),
        ("lfo", 8),
        ("filter", 7),
        ("shaper", 3),
        ("global", 10),
    ];
    for (cluster, params) in param_counts {
        let mapped = extracted
            .param_branch
            .values()
            .filter(|c| c.as_str() == cluster)
            .count();
        assert_eq!(mapped, params, "param count for {cluster}");
        let b = branch(&extracted, cluster);
        assert!(
            b.weight >= params,
            "weight of {cluster} ({}) below its param count ({params})",
            b.weight,
        );
        // children carry the owned-def heft: their weights sum to it.
        let child_sum: usize = b.children.iter().map(|c| c.weight).sum();
        assert_eq!(
            b.weight,
            params + child_sum,
            "weight identity for {cluster}"
        );
    }
    // Per-op level/coarse/fine pre-resolves are owned defs, so op branches
    // outweigh their raw param count.
    assert!(branch(&extracted, "opa").weight > 13);
    // Algorithm edge selectors + glide/spread chain belong to global.
    assert!(branch(&extracted, "global").weight > 20);
}

// ── wavetable / triton ──

#[test]
fn wavetable_cluster_names_and_order() {
    let extracted = extract_skeleton(&instrument_source("core/wavetable/dsp.lisp"));
    assert_eq!(
        cluster_names(&extracted.skeleton),
        vec!["osc1", "osc2", "filter", "global", "amp", "filt"],
    );
    assert_eq!(
        extracted.param_branch.get("cutoff").map(String::as_str),
        Some("global"),
    );
    assert_eq!(
        extracted
            .param_branch
            .get("filter_env_amt")
            .map(String::as_str),
        Some("filter"),
    );
    // Oscillator branches own their def chains (phase/warp/fold/scan).
    assert!(branch(&extracted, "osc1").weight > 7);
    assert!(!branch(&extracted, "osc1").children.is_empty());
}

#[test]
fn triton_extracts_within_target_granularity() {
    let extracted = extract_skeleton(&instrument_source("core/triton/dsp.lisp"));
    let names = cluster_names(&extracted.skeleton);
    for expected in [
        "osc1", "osc2", "feg", "aeg", "lfo1", "lfo2", "peg", "global",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    let count = extracted.skeleton.branches.len();
    assert!(
        (8..=MAX_BRANCHES).contains(&count),
        "triton branch count {count} outside 8..=30",
    );
}

// ── stability / determinism ──

#[test]
fn whitespace_and_comment_edits_do_not_change_skeleton() {
    for rel in [
        "core/operator/dsp.lisp",
        "core/wavetable/dsp.lisp",
        "core/triton/dsp.lisp",
    ] {
        let source = instrument_source(rel);
        let baseline = extract_skeleton(&source);

        let mut edited = String::from("; leading comment added\n;; another\n");
        edited.push_str(&source.replace("\n(", "\n\n\t  ("));
        edited.push_str("\n   ; trailing comment\n\n");
        let reextracted = extract_skeleton(&edited);

        assert_eq!(baseline, reextracted, "skeleton changed for {rel}");
    }
}

#[test]
fn extraction_is_deterministic_byte_identical() {
    for rel in [
        "core/operator/dsp.lisp",
        "core/wavetable/dsp.lisp",
        "core/triton/dsp.lisp",
    ] {
        let source = instrument_source(rel);
        let a = format!("{:?}", extract_skeleton(&source));
        let b = format!("{:?}", extract_skeleton(&source));
        assert_eq!(a, b, "nondeterministic extraction for {rel}");
    }
}

// ── merge path ──

fn many_cluster_source(clusters: usize) -> String {
    let mut src = String::new();
    for i in 0..clusters {
        src.push_str(&format!("(param c{i:02}_a @default 0 @min 0 @max 1)\n"));
        src.push_str(&format!("(param c{i:02}_b @default 0 @min 0 @max 1)\n"));
    }
    src.push_str("(out 0 1 @name audio)\n");
    src
}

#[test]
fn over_thirty_clusters_merge_smallest_first_deterministically() {
    let source = many_cluster_source(40);
    let extracted = extract_skeleton(&source);
    assert_eq!(extracted.skeleton.branches.len(), MAX_BRANCHES);

    // All 40 clusters weigh 2, so smallest-first-then-name pairs them up in
    // name order until the cap: c00+c01 .. c18+c19, then 20 untouched.
    let names = cluster_names(&extracted.skeleton);
    for pair in 0..10 {
        let merged = format!("c{:02}+c{:02}", pair * 2, pair * 2 + 1);
        assert_eq!(names[pair], merged.as_str());
    }
    assert_eq!(names[10], "c20");
    assert_eq!(names.last(), Some(&"c39"));

    // Weight and param mapping survive the merge.
    let total: usize = extracted.skeleton.branches.iter().map(|b| b.weight).sum();
    assert_eq!(total, 80);
    assert_eq!(
        extracted.param_branch.get("c01_b").map(String::as_str),
        Some("c00+c01"),
    );
    assert_eq!(
        extracted.param_branch.get("c39_a").map(String::as_str),
        Some("c39"),
    );

    // Deterministic across runs.
    let again = format!("{:?}", extract_skeleton(&source));
    assert_eq!(format!("{extracted:?}"), again);
}

#[test]
fn exactly_thirty_clusters_do_not_merge() {
    let extracted = extract_skeleton(&many_cluster_source(30));
    assert_eq!(extracted.skeleton.branches.len(), 30);
    assert!(cluster_names(&extracted.skeleton)
        .iter()
        .all(|n| !n.contains('+')));
}

// ── clustering edge cases ──

#[test]
fn param_extending_another_param_name_joins_its_cluster() {
    let source = "(param filter @default 0 @min 0 @max 1)\n\
                  (param filter_freq @default 100 @min 20 @max 20000)\n\
                  (param tone @default 0 @min 0 @max 1)\n";
    let extracted = extract_skeleton(source);
    assert_eq!(cluster_names(&extracted.skeleton), vec!["filter", "global"],);
    assert_eq!(
        extracted.param_branch.get("filter").map(String::as_str),
        Some("filter"),
    );
    assert_eq!(
        extracted.param_branch.get("tone").map(String::as_str),
        Some("global"),
    );
}

#[test]
fn sub_prefix_cluster_folds_into_parent_cluster() {
    // env_loop_* would form its own cluster, but a parent env cluster exists
    // (anchored by env_sync), so it folds in — mirrors lfo_to_* joining lfo.
    let source = "(param env_loop_rate @default 1 @min 0 @max 10)\n\
                  (param env_loop_depth @default 0 @min 0 @max 1)\n\
                  (param env_sync @default 0 @min 0 @max 1)\n";
    let extracted = extract_skeleton(source);
    assert_eq!(cluster_names(&extracted.skeleton), vec!["env"]);
    assert_eq!(
        extracted
            .param_branch
            .get("env_loop_rate")
            .map(String::as_str),
        Some("env"),
    );
    assert_eq!(
        extracted.param_branch.get("env_sync").map(String::as_str),
        Some("env"),
    );
}

#[test]
fn sub_prefix_clusters_without_a_parent_stay_split() {
    let source = "(param env_loop_rate @default 1 @min 0 @max 10)\n\
                  (param env_loop_depth @default 0 @min 0 @max 1)\n\
                  (param env_sync_div @default 0 @min 0 @max 5)\n\
                  (param env_sync_rate @default 1 @min 0 @max 10)\n";
    let extracted = extract_skeleton(source);
    assert_eq!(
        cluster_names(&extracted.skeleton),
        vec!["env_loop", "env_sync"],
    );
}

#[test]
fn comment_only_source_yields_empty_skeleton() {
    // No forms at all — nothing to derive an identity from.
    let extracted = extract_skeleton("; just a comment\n");
    assert!(extracted.skeleton.branches.is_empty());
    assert!(extracted.param_branch.is_empty());
}

// ── never-empty invariant (param-less sources) ──

#[test]
fn defs_only_source_yields_def_branches() {
    let source = "(def osc (sine 220))\n\
                  (def mix (* osc 0.5))\n\
                  (defmacro helper (x) (* x 2))\n";
    let extracted = extract_skeleton(source);
    // Weight-bearing defs become branches in source order; macros carry none.
    assert_eq!(cluster_names(&extracted.skeleton), vec!["osc", "mix"]);
    assert!(extracted.param_branch.is_empty());
    assert!(!identity_branches(&extracted.skeleton).is_empty());
}

#[test]
fn param_less_def_less_source_still_gets_a_branch() {
    let extracted = extract_skeleton("(out 0 1 @name audio)\n");
    assert_eq!(extracted.skeleton.branches.len(), 1);
    let name = extracted.skeleton.branches[0].cluster.as_str();
    assert!(name.starts_with("src_"), "fallback branch name {name}");
    assert!(!identity_branches(&extracted.skeleton).is_empty());
    // Distinct sources keep distinct fallback identities.
    let other = extract_skeleton("(out 0 2 @name audio)\n");
    assert_ne!(name, other.skeleton.branches[0].cluster.as_str());
    // Deterministic across runs.
    let again = extract_skeleton("(out 0 1 @name audio)\n");
    assert_eq!(extracted, again);
}

// ── parser robustness ──

#[test]
fn deeply_nested_source_does_not_overflow() {
    // ~100k nested parens: the reader's depth cap must truncate gracefully
    // instead of blowing the stack.
    let mut source = String::from("(param depth_a @min 0 @max 1)\n(def deep ");
    source.push_str(&"(".repeat(100_000));
    source.push_str("depth_a");
    source.push_str(&")".repeat(100_000));
    source.push(')');
    let extracted = extract_skeleton(&source);
    assert!(!extracted.skeleton.branches.is_empty());
    let _ = param_specs(&source);

    // Unbalanced deep opens survive too.
    let unbalanced = "(".repeat(100_000);
    let _ = extract_skeleton(&unbalanced);
}

// ── symbol / prefix edge cases ──

#[test]
fn defs_named_like_float_literals_stay_in_reference_graph() {
    // `inf` / `nan` parse as f64 but are legitimate symbol names; they must
    // keep flowing through the def-reference graph.
    let source = "(param lfo_rate @min 0 @max 1)\n\
                  (param lfo_depth @min 0 @max 1)\n\
                  (def inf (* lfo_rate 2))\n\
                  (def nan (+ inf lfo_depth))\n";
    let extracted = extract_skeleton(source);
    // 2 params + the inf/nan owned-def chain.
    assert_eq!(branch(&extracted, "lfo").weight, 4);
}

#[test]
fn leading_underscore_params_do_not_form_an_empty_cluster() {
    let source = "(param _rate @min 0 @max 1)\n\
                  (param _depth @min 0 @max 1)\n";
    let extracted = extract_skeleton(source);
    assert!(extracted
        .skeleton
        .branches
        .iter()
        .all(|b| !b.cluster.is_empty()));
    // `_rate`/`_depth` share no non-empty prefix → singletons → global.
    assert_eq!(cluster_names(&extracted.skeleton), vec![GLOBAL_CLUSTER]);
}

// ── stock skeletons (builtins) ──

#[test]
fn stock_skeletons_cover_all_builtins_and_sampler() {
    let mut names: Vec<&str> = EffectDescriptor::builtin_insert_names().to_vec();
    names.push("sampler");
    for name in names {
        let descriptor = if name == "sampler" {
            EffectDescriptor::builtin_sampler()
        } else {
            EffectDescriptor::builtin_insert(name).unwrap()
        };
        let extracted = stock_skeleton(&descriptor);
        assert!(
            !extracted.skeleton.branches.is_empty(),
            "{name}: no branches",
        );
        assert!(
            extracted.skeleton.branches.len() <= MAX_BRANCHES,
            "{name}: over branch cap",
        );
        // Every param maps to an existing branch.
        assert_eq!(
            extracted.param_branch.len(),
            descriptor.params.len(),
            "{name}"
        );
        for (param, cluster) in &extracted.param_branch {
            assert!(
                extracted
                    .skeleton
                    .branches
                    .iter()
                    .any(|b| &b.cluster == cluster),
                "{name}: param {param} maps to unknown branch {cluster}",
            );
        }
        // Generic radial: weights are param counts, no def children.
        let total: usize = extracted.skeleton.branches.iter().map(|b| b.weight).sum();
        assert_eq!(total, descriptor.params.len(), "{name}: weight total");
        assert!(
            extracted
                .skeleton
                .branches
                .iter()
                .all(|b| b.children.is_empty()),
            "{name}: stock skeletons are flat",
        );
        // Deterministic.
        let again = format!("{:?}", stock_skeleton(&descriptor));
        assert_eq!(format!("{extracted:?}"), again, "{name}: nondeterministic");
    }
}

// ── geometry (P2) ──

fn norm_values(
    extracted: &ExtractedSkeleton,
    fill: f32,
) -> std::collections::BTreeMap<String, f32> {
    extracted
        .param_branch
        .keys()
        .map(|p| (p.clone(), fill))
        .collect()
}

#[test]
fn geometry_is_deterministic() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let values = norm_values(&extracted, 0.37);
    let a = resolve_geometry(&extracted, &values);
    let b = resolve_geometry(&extracted, &values);
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    assert!(!a.strokes.is_empty());
    assert!(!a.marks.is_empty());
}

#[test]
fn geometry_stays_inside_unit_square() {
    for rel in [
        "core/operator/dsp.lisp",
        "core/wavetable/dsp.lisp",
        "core/triton/dsp.lisp",
    ] {
        let extracted = extract_skeleton(&instrument_source(rel));
        for fill in [0.0, 0.5, 1.0] {
            let geom = resolve_geometry(&extracted, &norm_values(&extracted, fill));
            for stroke in &geom.strokes {
                assert!(stroke.points.len() >= 2, "{rel}: degenerate stroke");
                for p in &stroke.points {
                    assert!(
                        (-0.02..=1.02).contains(&p[0]) && (-0.02..=1.02).contains(&p[1]),
                        "{rel} fill {fill}: point {p:?} of {} escapes unit square",
                        stroke.branch,
                    );
                }
            }
            for mark in &geom.marks {
                assert!(mark.radius > 0.0, "{rel}: zero-radius mark {}", mark.param);
            }
        }
    }
}

#[test]
fn geometry_branch_lengths_never_degenerate() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let geom = resolve_geometry(&extracted, &norm_values(&extracted, 0.0));
    for stroke in &geom.strokes {
        let len: f32 = stroke
            .points
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
            .sum();
        assert!(len > 0.02, "stroke {} too short: {len}", stroke.branch);
    }
}

#[test]
fn changing_one_param_only_moves_its_own_branch() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let base_values = norm_values(&extracted, 0.5);
    let mut edited_values = base_values.clone();
    edited_values.insert("filter_freq".to_string(), 1.0);
    let owner = extracted.param_branch.get("filter_freq").unwrap().clone();

    let base = resolve_geometry(&extracted, &base_values);
    let edited = resolve_geometry(&extracted, &edited_values);

    assert_eq!(base.strokes.len(), edited.strokes.len());
    assert_eq!(base.marks.len(), edited.marks.len());
    let mut owner_changed = false;
    for (a, b) in base.strokes.iter().zip(&edited.strokes) {
        assert_eq!(a.branch, b.branch);
        if a.branch == owner {
            owner_changed |= a != b;
        } else {
            assert_eq!(a, b, "non-owner stroke {} moved", a.branch);
        }
    }
    for (a, b) in base.marks.iter().zip(&edited.marks) {
        assert_eq!((&a.branch, &a.param), (&b.branch, &b.param));
        if a.branch != owner {
            assert_eq!(a, b, "non-owner mark {} moved", a.param);
        }
    }
    assert!(owner_changed, "owning branch {owner} did not change");
    // The edited param's own mark grew.
    let mark = |g: &GlyphGeometry| {
        g.marks
            .iter()
            .find(|m| m.param == "filter_freq")
            .unwrap()
            .radius
    };
    assert!(mark(&edited) > mark(&base));
}

#[test]
fn param_ranges_reads_min_max() {
    let ranges = param_ranges(&instrument_source("core/operator/dsp.lisp"));
    assert_eq!(ranges.get("opa_level_db"), Some(&(-60.0, 0.0)));
    assert_eq!(ranges.get("opa_freq_hz"), Some(&(0.1, 20000.0)));
    assert_eq!(ranges.get("opa_on"), Some(&(0.0, 1.0)));
    assert_eq!(ranges.len(), 110);
}

#[test]
fn param_specs_sanitizes_degenerate_bounds() {
    let source = "(param a @min inf @max 1 @default 0.5)\n\
                  (param b @min 2 @max 2 @default 2)\n\
                  (param c @min 5 @max 1)\n\
                  (param d @min -1 @max nan @default nan)\n";
    let specs = param_specs(source);
    assert_eq!(specs.len(), 4);
    for (name, spec) in &specs {
        assert!(
            spec.min.is_finite() && spec.max.is_finite() && spec.default.is_finite(),
            "{name}: non-finite spec {spec:?}",
        );
        assert!(spec.min < spec.max, "{name}: degenerate bounds {spec:?}");
        assert!(
            (spec.min..=spec.max).contains(&spec.default),
            "{name}: default out of bounds {spec:?}",
        );
    }
    // min == max (host normalization would divide by zero) resets to 0..1;
    // the declared default clamps into the sanitized range.
    assert_eq!(
        specs.get("b"),
        Some(&ParamSpec {
            min: 0.0,
            max: 1.0,
            default: 1.0,
        }),
    );
}

#[test]
fn non_finite_values_do_not_poison_geometry() {
    let extracted = extract_skeleton(&instrument_source("core/operator/dsp.lisp"));
    let mut values = norm_values(&extracted, f32::NAN);
    values.insert("filter_freq".to_string(), f32::INFINITY);
    values.insert("opa_attack".to_string(), f32::NEG_INFINITY);
    let geom = resolve_geometry(&extracted, &values);
    for stroke in &geom.strokes {
        assert!(stroke.width.is_finite(), "{}: NaN width", stroke.branch);
        for p in &stroke.points {
            assert!(
                p[0].is_finite() && p[1].is_finite(),
                "{}: non-finite point {p:?}",
                stroke.branch,
            );
        }
    }
    for mark in &geom.marks {
        assert!(
            mark.pos[0].is_finite() && mark.pos[1].is_finite() && mark.radius.is_finite(),
            "{}: non-finite mark",
            mark.param,
        );
    }
    // Every non-finite value reads as the missing-param default (0.5).
    let baseline = resolve_geometry(&extracted, &norm_values(&extracted, 0.5));
    assert_eq!(geom, baseline);
}
