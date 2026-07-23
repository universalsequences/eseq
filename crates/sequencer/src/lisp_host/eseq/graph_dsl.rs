/*!
Shared parsing helpers for the graph-mode s-expression DSL.

Small value coercions (`graph_keyword`, `graph_number`, `graph_timebase`,
`graph_list_items`, `graph_head_symbol`) and spec parsers for the pieces of a
graph-mode `def-sequencer` form: duration (`graph_parse_duration_spec`), swing
(`graph_parse_swing_spec`), node params/state/leak (`graph_parse_param_list`,
`graph_parse_state_list`, `graph_parse_leak`), and shape/topology/edge
distribution (`graph_parse_shape`, `graph_parse_topology`). Nothing here
registers natives; the siblings `graph_manifest`, `graph_authoring`, and
`graph_update` build on these parsers.
*/

use super::super::*;
use crate::graph::{
    GraphDurationSpec, GraphSwingSpec, LeakSpec, ParamSpec, ShapeSpec, StateSpec, Topology,
};

/// Clone a list value's items out of their cells, or `None` if not a list.
pub(in crate::lisp_host) fn graph_list_items(value: &EValue) -> Option<Vec<EValue>> {
    match value {
        EValue::List(items) => Some(items.iter().map(|i| i.borrow().clone()).collect()),
        _ => None,
    }
}

/// The lowercased head symbol of a sub-form (`def-node`, `edges`, `grid`, …).
pub(in crate::lisp_host) fn graph_head_symbol(items: &[EValue]) -> Option<String> {
    match items.first() {
        Some(EValue::Symbol(s)) => Some(s.trim_start_matches('@').to_ascii_lowercase()),
        _ => None,
    }
}

/// Normalize a keyword/symbol/string to a bare lowercase key (no leading `:`/`@`).
pub(in crate::lisp_host) fn graph_keyword(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(k) | EValue::Symbol(k) | EValue::String(k) => Some(
            k.trim_start_matches('@')
                .trim_start_matches(':')
                .to_ascii_lowercase(),
        ),
        _ => None,
    }
}

pub(in crate::lisp_host) fn graph_number(value: &EValue) -> Option<f64> {
    match value {
        EValue::Number(n) => Some(*n),
        _ => None,
    }
}

pub(in crate::lisp_host) fn graph_symbol_string(value: &EValue) -> String {
    match value {
        EValue::Symbol(s) | EValue::String(s) | EValue::Keyword(s) => s.clone(),
        _ => String::new(),
    }
}

pub(in crate::lisp_host) fn graph_timebase(value: &EValue) -> Result<Timebase, String> {
    parse_timebase_arg(std::slice::from_ref(value), 0)
}

/// `(bars 4)` → beats (assumes 4/4), `(beats 2)` → beats, bare number → beats.
pub(in crate::lisp_host) fn graph_bars_or_beats(value: &EValue) -> f64 {
    if let Some(items) = graph_list_items(value) {
        let n = items.get(1).and_then(graph_number).unwrap_or(0.0);
        match graph_head_symbol(&items).as_deref() {
            Some("bars") | Some("bar") => return n * 4.0,
            Some("beats") | Some("beat") => return n,
            _ => {}
        }
    }
    graph_number(value).unwrap_or(0.0)
}

pub(in crate::lisp_host) fn graph_parse_duration_spec(value: &EValue) -> Result<GraphDurationSpec, String> {
    if let Some(n) = graph_number(value) {
        return Ok(GraphDurationSpec::Beats { value: n.max(0.0) });
    }
    if graph_keyword(value).as_deref() == Some("seed") {
        return Ok(GraphDurationSpec::Seed);
    }
    let items = graph_list_items(value)
        .ok_or_else(|| "duration expects a number, :seed, or duration form".to_string())?;
    let head = graph_head_symbol(&items)
        .ok_or_else(|| "duration form expects a symbol head".to_string())?;
    match head.as_str() {
        "steps" | "step" => {
            if items.len() != 2 {
                return Err("(steps n) expects exactly one numeric step count".to_string());
            }
            let value = items
                .get(1)
                .and_then(graph_number)
                .ok_or_else(|| "(steps n) expects a numeric step count".to_string())?;
            Ok(GraphDurationSpec::Steps {
                value: value.max(0.0),
            })
        }
        "beats" | "beat" => {
            if items.len() != 2 {
                return Err("(beats x) expects exactly one number or timebase".to_string());
            }
            let value = items
                .get(1)
                .ok_or_else(|| "(beats x) expects a number or timebase".to_string())?;
            if let Some(n) = graph_number(value) {
                Ok(GraphDurationSpec::Beats { value: n.max(0.0) })
            } else {
                Ok(GraphDurationSpec::Timebase {
                    index: graph_timebase(value)? as u8,
                })
            }
        }
        "delay" => {
            if items.len() != 1 {
                return Err("(delay) expects no arguments".to_string());
            }
            Ok(GraphDurationSpec::Delay)
        }
        "seed" => {
            if items.len() != 1 {
                return Err("(seed) expects no arguments".to_string());
            }
            Ok(GraphDurationSpec::Seed)
        }
        "+" => {
            if items.len() < 2 {
                return Err("(+ duration ...) expects at least one duration".to_string());
            }
            Ok(GraphDurationSpec::Add {
                items: items[1..]
                    .iter()
                    .map(graph_parse_duration_spec)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        "*" => {
            if items.len() != 3 {
                return Err("(* scalar duration) expects exactly two arguments".to_string());
            }
            let a = items
                .get(1)
                .ok_or_else(|| "(* scalar duration) expects a scalar".to_string())?;
            let b = items
                .get(2)
                .ok_or_else(|| "(* scalar duration) expects a duration".to_string())?;
            match (graph_number(a), graph_number(b)) {
                (Some(factor), None) => Ok(GraphDurationSpec::Mul {
                    factor,
                    item: Box::new(graph_parse_duration_spec(b)?),
                }),
                (None, Some(factor)) => Ok(GraphDurationSpec::Mul {
                    factor,
                    item: Box::new(graph_parse_duration_spec(a)?),
                }),
                _ => Err("duration * expects one numeric scalar and one duration".to_string()),
            }
        }
        "min" => {
            if items.len() < 2 {
                return Err("(min duration ...) expects at least one duration".to_string());
            }
            Ok(GraphDurationSpec::Min {
                items: items[1..]
                    .iter()
                    .map(graph_parse_duration_spec)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        "max" => {
            if items.len() < 2 {
                return Err("(max duration ...) expects at least one duration".to_string());
            }
            Ok(GraphDurationSpec::Max {
                items: items[1..]
                    .iter()
                    .map(graph_parse_duration_spec)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        other => Err(format!("unknown duration form `{other}`")),
    }
}

pub(in crate::lisp_host) fn graph_swing_resolution(value: &EValue) -> Result<u8, String> {
    let label = match value {
        EValue::Keyword(s) | EValue::Symbol(s) | EValue::String(s) => {
            s.trim_start_matches(':').to_ascii_lowercase()
        }
        EValue::Number(n) => {
            let index = (*n).round().max(0.0) as u8;
            return Ok(index.min(3));
        }
        _ => String::new(),
    };
    match label.as_str() {
        "16" | "1/16" | "sixteenth" => Ok(0),
        "8" | "1/8" | "eighth" => Ok(1),
        "4" | "1/4" | "quarter" => Ok(2),
        "2" | "1/2" | "half" => Ok(3),
        _ => Err("expects swing resolution :16, :8, :4, or :2".to_string()),
    }
}

pub(in crate::lisp_host) fn graph_swing_value(amount: f64, resolution: u8) -> EValue {
    lisp_list(vec![
        EValue::Symbol("swing".to_string()),
        EValue::Number(amount.clamp(50.0, 75.0)),
        EValue::Number(resolution as f64),
    ])
}

pub(in crate::lisp_host) fn graph_swing_value_from_args(args: &[EValue]) -> Result<EValue, String> {
    if !(1..=2).contains(&args.len()) {
        return Err("swing expects (swing pct [:16|:8|:4|:2])".to_string());
    }
    let amount = graph_number(&args[0]).ok_or_else(|| "swing expects a numeric pct".to_string())?;
    let resolution = match args.get(1) {
        Some(value) => graph_swing_resolution(value)?,
        None => 0,
    };
    Ok(graph_swing_value(amount, resolution))
}

pub(in crate::lisp_host) fn graph_parse_swing_spec(value: &EValue) -> Result<GraphSwingSpec, String> {
    if let Some(n) = graph_number(value) {
        return Ok(GraphSwingSpec::new(n as f32, 0));
    }
    let items = graph_list_items(value)
        .ok_or_else(|| "swing expects a number or (swing pct [:16|:8|:4|:2])".to_string())?;
    let head =
        graph_head_symbol(&items).ok_or_else(|| "swing form expects a symbol head".to_string())?;
    if head != "swing" {
        return Err(format!("unknown swing form `{head}`"));
    }
    if !(2..=3).contains(&items.len()) {
        return Err("(swing pct [:16|:8|:4|:2]) expects one or two arguments".to_string());
    }
    let amount = items
        .get(1)
        .and_then(graph_number)
        .ok_or_else(|| "(swing pct ...) expects a numeric pct".to_string())?;
    let resolution = match items.get(2) {
        Some(value) => graph_swing_resolution(value)?,
        None => 0,
    };
    Ok(GraphSwingSpec::new(amount as f32, resolution))
}

/// `(name :float min max :default d)` / `(name :int min max :default d)`.
pub(in crate::lisp_host) fn graph_parse_param(items: &[EValue]) -> Option<ParamSpec> {
    let name = match items.first()? {
        EValue::Symbol(s) | EValue::String(s) => s.clone(),
        _ => return None,
    };
    let mut is_int = false;
    let mut nums: Vec<f64> = Vec::new();
    let mut default: Option<f64> = None;
    let mut i = 1;
    while i < items.len() {
        match &items[i] {
            EValue::Keyword(k) => match k.trim_start_matches(':').to_ascii_lowercase().as_str() {
                "int" => is_int = true,
                "float" => is_int = false,
                "default" => {
                    i += 1;
                    default = items.get(i).and_then(graph_number);
                }
                _ => {}
            },
            EValue::Number(n) => nums.push(*n),
            _ => {}
        }
        i += 1;
    }
    let min = nums.first().copied().unwrap_or(0.0);
    let max = nums.get(1).copied().unwrap_or(0.0);
    Some(ParamSpec {
        name,
        min,
        max,
        default: default.unwrap_or(min),
        is_int,
    })
}

pub(in crate::lisp_host) fn graph_parse_param_list(value: &EValue) -> Vec<ParamSpec> {
    graph_list_items(value)
        .unwrap_or_default()
        .iter()
        .filter_map(graph_list_items)
        .filter_map(|items| graph_parse_param(&items))
        .collect()
}

/// `(per-step :energy-decay)` or `(per-step 0.9)`.
pub(in crate::lisp_host) fn graph_parse_leak(value: &EValue) -> Option<LeakSpec> {
    let items = graph_list_items(value)?;
    if graph_head_symbol(&items).as_deref() != Some("per-step") {
        return None;
    }
    match items.get(1) {
        Some(EValue::Number(n)) => Some(LeakSpec::PerStep(*n)),
        Some(v) if graph_keyword(v).as_deref() == Some("energy-decay") => {
            Some(LeakSpec::PerStepEnergyDecay)
        }
        _ => None,
    }
}

/// `(energy :leak (per-step :energy-decay))`.
pub(in crate::lisp_host) fn graph_parse_state(items: &[EValue]) -> Option<StateSpec> {
    let name = match items.first()? {
        EValue::Symbol(s) | EValue::String(s) => s.clone(),
        _ => return None,
    };
    let mut leak = None;
    let mut i = 1;
    while i < items.len() {
        if graph_keyword(&items[i]).as_deref() == Some("leak") {
            i += 1;
            if let Some(v) = items.get(i) {
                leak = graph_parse_leak(v);
            }
        }
        i += 1;
    }
    Some(StateSpec { name, leak })
}

pub(in crate::lisp_host) fn graph_parse_state_list(value: &EValue) -> Vec<StateSpec> {
    graph_list_items(value)
        .unwrap_or_default()
        .iter()
        .filter_map(graph_list_items)
        .filter_map(|items| graph_parse_state(&items))
        .collect()
}

fn graph_shape_count(value: &EValue, context: &str) -> Result<usize, String> {
    let n = graph_number(value).ok_or_else(|| format!("{context} expects a numeric count"))?;
    if !n.is_finite() {
        return Err(format!("{context} expects a finite count"));
    }
    let rounded = n.round();
    if rounded < 1.0 {
        return Err(format!("{context} expects a count >= 1"));
    }
    Ok(rounded as usize)
}

fn graph_parse_line_shape(items: &[EValue]) -> Result<ShapeSpec, String> {
    if items.len() == 2 {
        return Ok(ShapeSpec::Line(graph_shape_count(&items[1], "(line N)")?));
    }

    let mut default = None;
    let mut min = None;
    let mut max = None;
    let mut idx = 1;
    if let Some(value) = items.get(idx).and_then(graph_number) {
        if !value.is_finite() {
            return Err("(line DEFAULT :max MAX) expects a finite default".to_string());
        }
        let rounded = value.round();
        if rounded < 1.0 {
            return Err("(line DEFAULT :max MAX) expects a default >= 1".to_string());
        }
        default = Some(rounded as usize);
        idx += 1;
    }

    while idx < items.len() {
        let key = graph_keyword(&items[idx])
            .ok_or_else(|| "(line ...) expects keyword/value pairs after default".to_string())?;
        idx += 1;
        let value = items
            .get(idx)
            .ok_or_else(|| format!("(line ...) missing value for :{key}"))?;
        match key.as_str() {
            "default" => default = Some(graph_shape_count(value, "(line :default)")?),
            "min" => min = Some(graph_shape_count(value, "(line :min)")?),
            "max" => max = Some(graph_shape_count(value, "(line :max)")?),
            other => return Err(format!("line shape unknown key :{other}")),
        }
        idx += 1;
    }

    let default =
        default.ok_or_else(|| "(line :default D :max M) requires :default".to_string())?;
    let min = min.unwrap_or(1);
    let max = max.ok_or_else(|| "(line :default D :max M) requires :max".to_string())?;
    if min > max {
        return Err(format!(
            "line variable shape requires :min <= :max, got {min} > {max}"
        ));
    }
    if default < min || default > max {
        return Err(format!(
            "line variable shape requires default within [{min}, {max}], got {default}"
        ));
    }
    Ok(ShapeSpec::VariableLine { default, min, max })
}

pub(in crate::lisp_host) fn graph_parse_shape(value: &EValue) -> Result<ShapeSpec, String> {
    let items =
        graph_list_items(value).ok_or_else(|| ":shape expects a generator form".to_string())?;
    let n = |idx: usize| {
        items
            .get(idx)
            .and_then(graph_number)
            .map(|n| n.max(0.0) as usize)
    };
    match graph_head_symbol(&items).as_deref() {
        Some("grid") => Ok(ShapeSpec::Grid {
            rows: n(1).ok_or("(grid R C) expects rows")?,
            cols: n(2).ok_or("(grid R C) expects cols")?,
        }),
        Some("line") => graph_parse_line_shape(&items),
        Some("ring") => Ok(ShapeSpec::Ring(n(1).ok_or("(ring N) expects N")?)),
        other => Err(format!("unknown :shape generator: {other:?}")),
    }
}

pub(in crate::lisp_host) fn graph_parse_topology(value: &EValue) -> Result<Topology, String> {
    let items =
        graph_list_items(value).ok_or_else(|| ":topology expects a generator form".to_string())?;
    match graph_head_symbol(&items).as_deref() {
        Some("all-to-all") => Ok(Topology::AllToAll),
        Some(other) => Err(format!(
            "unsupported :topology `{other}` (v1 supports all-to-all)"
        )),
        None => Err(":topology expects a generator form".to_string()),
    }
}

pub(in crate::lisp_host) fn graph_edge_distribution(
    value: &EValue,
) -> Result<crate::graph::EdgeDistribution, String> {
    match graph_keyword(value).as_deref() {
        Some("broadcast") | Some("broadcast-weighted") | Some("all") => {
            Ok(crate::graph::EdgeDistribution::BroadcastWeighted)
        }
        Some("weighted-choice") | Some("choice") | Some("markov") => {
            Ok(crate::graph::EdgeDistribution::WeightedChoice)
        }
        other => Err(format!(
            "unsupported edge :distribution {other:?}; expected :broadcast-weighted or :weighted-choice"
        )),
    }
}
