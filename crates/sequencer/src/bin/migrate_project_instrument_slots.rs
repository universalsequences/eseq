use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sequencer::effects::EffectDescriptor;
use sequencer::lisp_effect;
use sequencer::project::{ProjectEffectSlot, ProjectFile, ProjectTrack};
use sequencer::sequencer::MAX_STEPS;

fn usage() {
    eprintln!("Usage: migrate_project_instrument_slots <input-project.json> <output-project.json>");
}

fn compile_instrument_descriptor(name: &str) -> Result<EffectDescriptor, String> {
    let source = lisp_effect::load_instrument_source(name)
        .map_err(|error| format!("failed to load instrument source for '{name}': {error}"))?;
    let asset_base = lisp_effect::instrument_source_path(name)
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let manifest_json =
        lisp_effect::compile_instrument_with_asset_base(&source, 44_100, asset_base.as_deref())?;
    let manifest = lisp_effect::parse_manifest(&manifest_json)?;
    Ok(lisp_effect::instrument_descriptor_from_manifest(
        name, &manifest,
    ))
}

fn migrate_slot(slot: &ProjectEffectSlot, desc: &EffectDescriptor) -> ProjectEffectSlot {
    let new_param_count = desc.params.len();
    let old_idx_by_node = slot
        .param_node_indices
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, node)| (node, idx))
        .collect::<HashMap<_, _>>();
    let old_generated_start = slot
        .param_node_indices
        .iter()
        .rposition(|node| *node >= sequencer::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE)
        .map(|idx| idx + 1)
        .unwrap_or_else(|| {
            slot.param_node_indices
                .iter()
                .position(|node| *node == DGEN_ENABLED_PARAM_IDX)
                .map(|idx| idx + 1)
                .unwrap_or(slot.defaults.len())
        });
    let old_generated_idx_by_node = slot
        .param_node_indices
        .iter()
        .copied()
        .enumerate()
        .skip(old_generated_start)
        .map(|(idx, node)| (node, idx))
        .collect::<HashMap<_, _>>();
    let enabled_idx = desc
        .params
        .iter()
        .position(|param| param.name.eq_ignore_ascii_case("enabled"));

    let old_idx_for = |new_idx: usize| {
        if Some(new_idx) == enabled_idx {
            return old_idx_by_node.get(&DGEN_ENABLED_PARAM_IDX).copied();
        }

        let param = &desc.params[new_idx];
        if is_generated_mod_runtime_param_name(&param.name) {
            return old_generated_idx_by_node
                .get(&param.node_param_idx)
                .copied();
        }

        if enabled_idx.is_some_and(|idx| new_idx < idx) {
            let ordinal_idx = (new_idx < slot.defaults.len()).then_some(new_idx);
            let node_idx = old_idx_by_node.get(&param.node_param_idx).copied();
            return match (node_idx, ordinal_idx) {
                (Some(node_idx), Some(ordinal_idx)) => {
                    let node_value = slot
                        .defaults
                        .get(node_idx)
                        .copied()
                        .unwrap_or(param.default);
                    let ordinal_value = slot
                        .defaults
                        .get(ordinal_idx)
                        .copied()
                        .unwrap_or(param.default);
                    if value_in_param_range(node_value, param)
                        && !value_in_param_range(ordinal_value, param)
                    {
                        Some(node_idx)
                    } else {
                        Some(ordinal_idx)
                    }
                }
                (Some(node_idx), None) => Some(node_idx),
                (None, Some(ordinal_idx)) => Some(ordinal_idx),
                (None, None) => None,
            };
        }

        old_idx_by_node.get(&param.node_param_idx).copied()
    };

    let mut defaults = desc
        .params
        .iter()
        .map(|param| param.default)
        .collect::<Vec<_>>();
    for new_idx in 0..new_param_count {
        if let Some(old_idx) = old_idx_for(new_idx) {
            if let Some(value) = slot.defaults.get(old_idx).copied() {
                let param = &desc.params[new_idx];
                defaults[new_idx] = value.clamp(param.min.min(param.max), param.min.max(param.max));
            }
        }
    }
    apply_output_safety_defaults(&mut defaults, desc);

    let plocks = vec![vec![None; new_param_count]; MAX_STEPS];

    let mut migrated = ProjectEffectSlot {
        num_params: new_param_count as u32,
        defaults,
        plocks,
        param_node_indices: desc
            .params
            .iter()
            .map(|param| param.node_param_idx)
            .collect(),
        param_node_spans: desc
            .params
            .iter()
            .map(|param| param.node_param_span.max(1))
            .collect(),
        ir: slot.ir.clone(),
    };
    recompute_modulation_active_params(&mut migrated, desc);
    migrated
}

const DGEN_ENABLED_PARAM_IDX: u32 = 4;

fn value_in_param_range(value: f32, param: &sequencer::effects::ParamDescriptor) -> bool {
    let min = param.min.min(param.max);
    let max = param.min.max(param.max);
    value >= min && value <= max
}

fn apply_output_safety_defaults(defaults: &mut [f32], desc: &EffectDescriptor) {
    for (idx, param) in desc.params.iter().enumerate() {
        if param.name.eq_ignore_ascii_case("gain")
            && defaults.get(idx).copied().unwrap_or(param.default).abs() <= f32::EPSILON
            && param.default.abs() > f32::EPSILON
        {
            defaults[idx] = param.default;
        }
    }
}

fn is_generated_mod_runtime_param_name(name: &str) -> bool {
    name.starts_with("__host_mod__")
        || name.starts_with("__dgen_mod_active__")
        || (name.starts_with("mod ") && name.contains(" slot ") && name.ends_with(" amt"))
}

fn recompute_modulation_active_params(slot: &mut ProjectEffectSlot, desc: &EffectDescriptor) {
    let mut active_indices = desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| target.active_param_idx)
        .collect::<Vec<_>>();
    active_indices.sort_unstable();
    active_indices.dedup();

    for active_idx in active_indices {
        if active_idx >= slot.defaults.len() {
            continue;
        }

        let group = desc
            .instrument_modulation_targets
            .iter()
            .filter(|target| target.active_param_idx == Some(active_idx))
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }

        let default_active = group.iter().any(|target| {
            slot.defaults
                .get(target.depth_param_idx)
                .copied()
                .unwrap_or(0.0)
                .abs()
                > f32::EPSILON
        });
        slot.defaults[active_idx] = if default_active { 1.0 } else { 0.0 };

        for step in 0..MAX_STEPS {
            let Some(step_plocks) = slot.plocks.get_mut(step) else {
                continue;
            };
            if active_idx >= step_plocks.len() {
                continue;
            }

            let has_depth_plock = group.iter().any(|target| {
                step_plocks
                    .get(target.depth_param_idx)
                    .copied()
                    .flatten()
                    .is_some()
            });
            if has_depth_plock {
                let active = group.iter().any(|target| {
                    step_plocks
                        .get(target.depth_param_idx)
                        .copied()
                        .flatten()
                        .unwrap_or_else(|| {
                            slot.defaults
                                .get(target.depth_param_idx)
                                .copied()
                                .unwrap_or(0.0)
                        })
                        .abs()
                        > f32::EPSILON
                });
                step_plocks[active_idx] = Some(if active { 1.0 } else { 0.0 });
            } else {
                step_plocks[active_idx] = None;
            }
        }
    }
}

fn migrate_project(mut project: ProjectFile) -> Result<ProjectFile, String> {
    let mut descriptors = HashMap::<String, EffectDescriptor>::new();
    let custom_tracks = project
        .tracks
        .iter()
        .enumerate()
        .filter_map(|(track_idx, track)| match track {
            ProjectTrack::Custom {
                instrument_name, ..
            } => Some((track_idx, instrument_name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (_, instrument_name) in &custom_tracks {
        if !descriptors.contains_key(instrument_name) {
            descriptors.insert(
                instrument_name.clone(),
                compile_instrument_descriptor(instrument_name)?,
            );
        }
    }

    for pattern in &mut project.patterns {
        for (track_idx, instrument_name) in &custom_tracks {
            let Some(slot) = pattern.instrument_slots.get_mut(*track_idx) else {
                continue;
            };
            let desc = descriptors
                .get(instrument_name)
                .ok_or_else(|| format!("missing descriptor for '{instrument_name}'"))?;
            *slot = migrate_slot(slot, desc);
        }
    }

    Ok(project)
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        usage();
        std::process::exit(2);
    }

    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let input = input.canonicalize().unwrap_or(input);
    let output = if output.is_absolute() {
        output
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(output)
    };

    if let Err(error) = sequencer::paths::enter_sequencer_dir() {
        eprintln!("failed to enter sequencer crate directory: {error}");
        std::process::exit(1);
    }

    let source = match std::fs::read_to_string(&input) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read '{}': {error}", input.display());
            std::process::exit(1);
        }
    };
    let mut project = match serde_json::from_str::<ProjectFile>(&source) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("failed to parse '{}': {error}", input.display());
            std::process::exit(1);
        }
    };
    if let Some(stem) = output.file_stem().and_then(|stem| stem.to_str()) {
        project.name = stem.to_string();
    }

    let migrated = match migrate_project(project) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("migration failed: {error}");
            std::process::exit(1);
        }
    };
    let json = match serde_json::to_string(&migrated) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("failed to serialize migrated project: {error}");
            std::process::exit(1);
        }
    };
    if let Some(parent) = output.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("failed to create '{}': {error}", parent.display());
            std::process::exit(1);
        }
    }
    if let Err(error) = std::fs::write(&output, json) {
        eprintln!("failed to write '{}': {error}", output.display());
        std::process::exit(1);
    }
    println!("wrote {}", output.display());
}
