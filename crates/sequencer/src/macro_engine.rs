//! Project-global live parameter overrides.
//!
//! Macros sit between persisted scene defaults and the DSP live-send path.
//! Scheduler-time p-locks and process writes remain more specific. The command
//! thread publishes this table into immutable scheduler snapshots, where the
//! macro value becomes the effective default beneath explicit p-locks and
//! authored process writes.

use std::collections::HashMap;

use crate::neural::ParamNodeId;
use crate::process::ParamTarget;

pub type MacroId = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StealQuantize {
    Off,
    Sixteenth,
    #[default]
    Bar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneMacroConfig {
    pub target_scene: usize,
    pub morph_params: bool,
    pub steal_patterns: bool,
    pub quantize: StealQuantize,
    pub track_mask: Option<Vec<bool>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroKind {
    Mapped,
    Scene(SceneMacroConfig),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroMapping {
    pub track: usize,
    pub target: ParamTarget,
    pub range_min: f32,
    pub range_max: f32,
    pub curve: MacroCurve,
    /// Suspended mappings remain part of the macro but do not own an override.
    pub suspended: bool,
    resolved_key: Option<MacroParamKey>,
}

impl MacroMapping {
    pub fn new(
        track: usize,
        target: ParamTarget,
        range_min: f32,
        range_max: f32,
        curve: MacroCurve,
    ) -> Result<Self, MacroEngineError> {
        Self::new_resolved(track, target, None, range_min, range_max, curve)
    }

    /// Constructs a mapping whose descriptor index has already been resolved.
    /// The index is used only when the live node has no stable `ParamNodeId`.
    pub fn new_resolved(
        track: usize,
        target: ParamTarget,
        param_idx: Option<usize>,
        range_min: f32,
        range_max: f32,
        curve: MacroCurve,
    ) -> Result<Self, MacroEngineError> {
        if matches!(
            target,
            ParamTarget::StepParam { .. } | ParamTarget::ProcessInlet { .. }
        ) {
            return Err(MacroEngineError::UnsupportedTarget);
        }
        if !range_min.is_finite() || !range_max.is_finite() {
            return Err(MacroEngineError::NonFiniteRange);
        }

        let resolved_key = MacroParamKey::from_target(track, &target, param_idx);
        Ok(Self {
            track,
            target,
            range_min,
            range_max,
            curve,
            suspended: resolved_key.is_none(),
            resolved_key,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Macro {
    pub id: MacroId,
    /// Immutable, project-unique identity used by declarative scripts.
    pub key: Option<String>,
    pub name: String,
    pub value: f32,
    pub mappings: Vec<MacroMapping>,
    pub kind: MacroKind,
    last_write_order: Option<u64>,
}

impl Macro {
    pub fn new(id: MacroId, name: impl Into<String>, kind: MacroKind) -> Self {
        Self {
            id,
            key: None,
            name: name.into(),
            value: 0.0,
            mappings: Vec::new(),
            kind,
            last_write_order: None,
        }
    }
}

/// Stable identity for one live parameter.
///
/// Effect and instrument parameters use `ParamNodeId` whenever the live node
/// exposes one. The index variants are the fallback required for nodes without
/// a stable identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MacroParamKey {
    Node {
        track: usize,
        param_id: ParamNodeId,
    },
    Instrument {
        track: usize,
        param: usize,
    },
    Effect {
        track: usize,
        slot: usize,
        param: usize,
    },
    MidiFx {
        track: usize,
        slot: usize,
        fx: String,
        param: String,
    },
    RackSlot {
        track: usize,
        slot: usize,
        param: String,
    },
    RackSlotInstrument {
        track: usize,
        slot: usize,
        param: String,
    },
}

impl MacroParamKey {
    pub fn for_instrument(track: usize, param_idx: usize, param_id: Option<ParamNodeId>) -> Self {
        param_id.map_or(
            Self::Instrument {
                track,
                param: param_idx,
            },
            |param_id| Self::Node { track, param_id },
        )
    }

    pub fn for_effect(
        track: usize,
        slot: usize,
        param_idx: usize,
        param_id: Option<ParamNodeId>,
    ) -> Self {
        param_id.map_or(
            Self::Effect {
                track,
                slot,
                param: param_idx,
            },
            |param_id| Self::Node { track, param_id },
        )
    }

    pub fn from_target(
        track: usize,
        target: &ParamTarget,
        param_idx: Option<usize>,
    ) -> Option<Self> {
        match target {
            ParamTarget::InstrumentParam { param_id, .. } => param_id
                .map(|param_id| Self::Node { track, param_id })
                .or_else(|| param_idx.map(|param| Self::Instrument { track, param })),
            ParamTarget::EffectParam { slot, param_id, .. } => param_id
                .map(|param_id| Self::Node { track, param_id })
                .or_else(|| {
                    param_idx.map(|param| Self::Effect {
                        track,
                        slot: *slot,
                        param,
                    })
                }),
            ParamTarget::MidiFxParam { slot, fx, param } => Some(Self::MidiFx {
                track,
                slot: *slot,
                fx: fx.clone(),
                param: param.clone(),
            }),
            ParamTarget::RackSlotParam { slot, param } => Some(Self::RackSlot {
                track,
                slot: *slot,
                param: param.clone(),
            }),
            ParamTarget::RackSlotInstrumentParam {
                slot,
                param,
                param_id,
            } => param_id
                .map(|param_id| Self::Node { track, param_id })
                .or_else(|| {
                    Some(Self::RackSlotInstrument {
                        track,
                        slot: *slot,
                        param: param.clone(),
                    })
                }),
            ParamTarget::StepParam { .. } | ParamTarget::ProcessInlet { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroEngineError {
    DuplicateMacroId(MacroId),
    DuplicateMacroKey(String),
    InvalidMacroKey,
    MacroIdExhausted,
    UnknownMacro(MacroId),
    UnknownMapping {
        macro_id: MacroId,
        mapping_idx: usize,
    },
    TargetAlreadyMapped {
        owner_id: MacroId,
    },
    UnsupportedTarget,
    NonFiniteValue,
    NonFiniteRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMacroTarget {
    pub target: ParamTarget,
    pub key: MacroParamKey,
}

#[derive(Clone, Copy, Debug)]
struct ActiveOverride {
    macro_id: MacroId,
    value: f32,
    write_order: u64,
}

#[derive(Debug)]
pub struct MacroEngine {
    macros: Vec<Macro>,
    next_id: MacroId,
    overrides: HashMap<MacroParamKey, f32>,
    next_write_order: u64,
}

impl Default for MacroEngine {
    fn default() -> Self {
        Self {
            macros: Vec::new(),
            next_id: 1,
            overrides: HashMap::new(),
            next_write_order: 1,
        }
    }
}

impl MacroEngine {
    pub fn macros(&self) -> &[Macro] {
        &self.macros
    }

    pub fn next_id(&self) -> MacroId {
        self.next_id
    }

    pub fn create_macro(
        &mut self,
        name: impl Into<String>,
        kind: MacroKind,
    ) -> Result<MacroId, MacroEngineError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(MacroEngineError::MacroIdExhausted)?;
        self.macros.push(Macro::new(id, name, kind));
        Ok(id)
    }

    /// Idempotently resolves or creates a script-authored mapped macro.
    /// Existing definitions are returned untouched so script re-evaluation
    /// cannot reset a user's name, mappings, ranges, or live value.
    pub fn ensure_macro(
        &mut self,
        key: impl AsRef<str>,
        initial_name: impl Into<String>,
    ) -> Result<MacroId, MacroEngineError> {
        let key = normalize_macro_key(key.as_ref())?;
        if let Some(existing) = self
            .macros
            .iter()
            .find(|macro_definition| macro_definition.key.as_deref() == Some(key.as_str()))
        {
            return Ok(existing.id);
        }

        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(MacroEngineError::MacroIdExhausted)?;
        let mut macro_definition = Macro::new(id, initial_name, MacroKind::Mapped);
        macro_definition.key = Some(key);
        self.macros.push(macro_definition);
        Ok(id)
    }

    pub fn insert_macro(&mut self, mut macro_definition: Macro) -> Result<(), MacroEngineError> {
        if self
            .macros
            .iter()
            .any(|existing| existing.id == macro_definition.id)
        {
            return Err(MacroEngineError::DuplicateMacroId(macro_definition.id));
        }
        if let Some(key) = macro_definition.key.as_deref() {
            let key = normalize_macro_key(key)?;
            if self
                .macros
                .iter()
                .any(|existing| existing.key.as_deref() == Some(key.as_str()))
            {
                return Err(MacroEngineError::DuplicateMacroKey(key));
            }
            macro_definition.key = Some(key);
        }
        let mut claimed = HashMap::<MacroParamKey, MacroId>::new();
        for existing in &self.macros {
            for mapping in &existing.mappings {
                if let Some(key) = mapping.resolved_key.clone() {
                    claimed.insert(key, existing.id);
                }
            }
        }
        for mapping in &macro_definition.mappings {
            if let Some(key) = mapping.resolved_key.clone() {
                if let Some(owner_id) = claimed.insert(key, macro_definition.id) {
                    return Err(MacroEngineError::TargetAlreadyMapped { owner_id });
                }
            }
        }
        self.next_id = self
            .next_id
            .max(macro_definition.id.checked_add(1).unwrap_or(MacroId::MAX));
        self.macros.push(macro_definition);
        Ok(())
    }

    /// Restores the persisted allocation cursor without permitting ID reuse.
    pub fn ensure_next_id_at_least(&mut self, next_id: MacroId) {
        self.next_id = self.next_id.max(next_id.max(1));
    }

    pub fn rename_macro(
        &mut self,
        id: MacroId,
        name: impl Into<String>,
    ) -> Result<(), MacroEngineError> {
        let Some(macro_definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        macro_definition.name = name.into();
        Ok(())
    }

    /// Deletes a macro and returns its targets so the App can re-send whichever
    /// owner (or base value) is revealed by the deletion.
    pub fn delete_macro(
        &mut self,
        id: MacroId,
    ) -> Result<Vec<(usize, ParamTarget)>, MacroEngineError> {
        let Some(index) = self.macros.iter().position(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        let removed = self.macros.remove(index);
        let touched = removed
            .mappings
            .into_iter()
            .filter(|mapping| !mapping.suspended && mapping.resolved_key.is_some())
            .map(|mapping| (mapping.track, mapping.target))
            .collect();
        self.rebuild_ownership();
        Ok(touched)
    }

    pub fn add_mapping(
        &mut self,
        id: MacroId,
        mapping: MacroMapping,
    ) -> Result<(), MacroEngineError> {
        if let Some(key) = mapping.resolved_key.as_ref() {
            if let Some(owner_id) = self.mapping_owner(key) {
                return Err(MacroEngineError::TargetAlreadyMapped { owner_id });
            }
        }
        let Some(index) = self.macros.iter().position(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        self.macros[index].mappings.push(mapping);
        if matches!(self.macros[index].kind, MacroKind::Mapped)
            && self.macros[index].last_write_order.is_none()
        {
            let write_order = self.allocate_write_order();
            self.macros[index].last_write_order = Some(write_order);
        }
        self.rebuild_ownership();
        Ok(())
    }

    fn mapping_owner(&self, key: &MacroParamKey) -> Option<MacroId> {
        self.macros.iter().find_map(|macro_definition| {
            macro_definition
                .mappings
                .iter()
                .any(|mapping| mapping.resolved_key.as_ref() == Some(key))
                .then_some(macro_definition.id)
        })
    }

    pub fn set_mapping_range(
        &mut self,
        id: MacroId,
        mapping_idx: usize,
        min: f32,
        max: f32,
    ) -> Result<Vec<(usize, ParamTarget)>, MacroEngineError> {
        if !min.is_finite() || !max.is_finite() {
            return Err(MacroEngineError::NonFiniteRange);
        }
        let mapping = self.mapping_mut(id, mapping_idx)?;
        mapping.range_min = min;
        mapping.range_max = max;
        let touched = mapping_touch(mapping);
        self.rebuild_ownership();
        Ok(touched.into_iter().collect())
    }

    pub fn set_mapping_curve(
        &mut self,
        id: MacroId,
        mapping_idx: usize,
        curve: MacroCurve,
    ) -> Result<Vec<(usize, ParamTarget)>, MacroEngineError> {
        let mapping = self.mapping_mut(id, mapping_idx)?;
        mapping.curve = curve;
        let touched = mapping_touch(mapping);
        self.rebuild_ownership();
        Ok(touched.into_iter().collect())
    }

    pub fn remove_mapping(
        &mut self,
        id: MacroId,
        mapping_idx: usize,
    ) -> Result<Vec<(usize, ParamTarget)>, MacroEngineError> {
        let Some(macro_definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        if mapping_idx >= macro_definition.mappings.len() {
            return Err(MacroEngineError::UnknownMapping {
                macro_id: id,
                mapping_idx,
            });
        }
        let mapping = macro_definition.mappings.remove(mapping_idx);
        let touched = mapping_touch(&mapping).into_iter().collect();
        self.rebuild_ownership();
        Ok(touched)
    }

    fn mapping_mut(
        &mut self,
        id: MacroId,
        mapping_idx: usize,
    ) -> Result<&mut MacroMapping, MacroEngineError> {
        let Some(macro_definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        macro_definition
            .mappings
            .get_mut(mapping_idx)
            .ok_or(MacroEngineError::UnknownMapping {
                macro_id: id,
                mapping_idx,
            })
    }

    pub fn macro_definition(&self, id: MacroId) -> Option<&Macro> {
        self.macros.iter().find(|item| item.id == id)
    }

    pub fn macro_by_key(&self, key: &str) -> Option<&Macro> {
        let key = normalize_macro_key(key).ok()?;
        self.macros
            .iter()
            .find(|macro_definition| macro_definition.key.as_deref() == Some(key.as_str()))
    }

    pub fn override_value(&self, key: &MacroParamKey) -> Option<f32> {
        self.overrides.get(key).copied()
    }

    pub fn effective_value(&self, key: &MacroParamKey, base: f32) -> f32 {
        self.override_value(key).unwrap_or(base)
    }

    pub fn is_engaged(&self, id: MacroId) -> bool {
        self.macro_definition(id)
            .is_some_and(|macro_definition| macro_definition.last_write_order.is_some())
    }

    /// Captures the live override layer for publication into an immutable
    /// scheduler snapshot. The scheduler never reads or locks `MacroEngine`.
    pub fn override_snapshot(&self) -> HashMap<MacroParamKey, f32> {
        self.overrides.clone()
    }

    /// Updates one macro and returns every resolved target whose effective
    /// value must be sent again by the App layer.
    pub fn set_value(&mut self, id: MacroId, value: f32) -> Vec<(usize, ParamTarget)> {
        if !value.is_finite() {
            return Vec::new();
        }
        let value = value.clamp(0.0, 1.0);
        let Some(index) = self.macros.iter().position(|item| item.id == id) else {
            return Vec::new();
        };

        let write_order = Some(self.allocate_write_order());
        let touched = {
            let macro_definition = &mut self.macros[index];
            macro_definition.value = value;
            macro_definition.last_write_order = write_order;
            macro_definition
                .mappings
                .iter()
                .filter(|mapping| !mapping.suspended && mapping.resolved_key.is_some())
                .map(|mapping| (mapping.track, mapping.target.clone()))
                .collect()
        };
        self.rebuild_ownership();
        touched
    }

    /// Explicitly releases a momentary macro without overloading value `0`,
    /// which is a valid engaged minimum for continuous mapped macros.
    pub fn release(&mut self, id: MacroId) -> Vec<(usize, ParamTarget)> {
        let Some(macro_definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Vec::new();
        };
        macro_definition.last_write_order = None;
        let touched = macro_definition
            .mappings
            .iter()
            .filter(|mapping| !mapping.suspended && mapping.resolved_key.is_some())
            .map(|mapping| (mapping.track, mapping.target.clone()))
            .collect();
        self.rebuild_ownership();
        touched
    }

    fn allocate_write_order(&mut self) -> u64 {
        if self.next_write_order == u64::MAX {
            let mut engaged = self
                .macros
                .iter()
                .enumerate()
                .filter_map(|(index, macro_definition)| {
                    macro_definition
                        .last_write_order
                        .map(|order| (index, order))
                })
                .collect::<Vec<_>>();
            engaged.sort_unstable_by_key(|(_, order)| *order);
            for (new_order, (index, _)) in engaged.into_iter().enumerate() {
                self.macros[index].last_write_order = Some(new_order as u64 + 1);
            }
            self.next_write_order = self
                .macros
                .iter()
                .filter(|macro_definition| macro_definition.last_write_order.is_some())
                .count() as u64
                + 1;
        }
        let order = self.next_write_order;
        self.next_write_order += 1;
        order
    }

    /// Re-resolves every mapping against the current scene. Missing targets are
    /// retained as suspended mappings, and engaged macros are rebuilt onto the
    /// newly resolved identities without changing their writer precedence.
    pub fn revalidate_mappings<F>(&mut self, mut resolve: F) -> Vec<(usize, ParamTarget)>
    where
        F: FnMut(usize, &ParamTarget) -> Option<ResolvedMacroTarget>,
    {
        let mut touched = Vec::new();
        for macro_definition in &mut self.macros {
            for mapping in &mut macro_definition.mappings {
                touched.push((mapping.track, mapping.target.clone()));
                if let Some(resolved) = resolve(mapping.track, &mapping.target) {
                    mapping.target = resolved.target;
                    mapping.resolved_key = Some(resolved.key);
                    mapping.suspended = false;
                } else {
                    mapping.resolved_key = None;
                    mapping.suspended = true;
                }
                touched.push((mapping.track, mapping.target.clone()));
            }
        }
        let mut claimed = HashMap::<MacroParamKey, MacroId>::new();
        for macro_definition in &mut self.macros {
            for mapping in &mut macro_definition.mappings {
                let Some(key) = mapping.resolved_key.clone() else {
                    continue;
                };
                if claimed.insert(key, macro_definition.id).is_some() {
                    mapping.resolved_key = None;
                    mapping.suspended = true;
                }
            }
        }
        self.rebuild_ownership();
        touched
    }

    fn rebuild_ownership(&mut self) {
        let mut owners: HashMap<MacroParamKey, Vec<ActiveOverride>> = HashMap::new();
        for macro_definition in &self.macros {
            let Some(write_order) = macro_definition.last_write_order else {
                continue;
            };
            for mapping in &macro_definition.mappings {
                if mapping.suspended {
                    continue;
                }
                let Some(key) = mapping.resolved_key.clone() else {
                    continue;
                };
                let value = lerp_curved(
                    mapping.range_min,
                    mapping.range_max,
                    macro_definition.value,
                    mapping.curve,
                );
                let entries = owners.entry(key).or_default();
                if let Some(existing) = entries
                    .iter_mut()
                    .find(|entry| entry.macro_id == macro_definition.id)
                {
                    existing.value = value;
                    existing.write_order = write_order;
                } else {
                    entries.push(ActiveOverride {
                        macro_id: macro_definition.id,
                        value,
                        write_order,
                    });
                }
            }
        }

        self.overrides = owners
            .iter()
            .filter_map(|(key, entries)| {
                entries
                    .iter()
                    .max_by_key(|entry| entry.write_order)
                    .map(|entry| (key.clone(), entry.value))
            })
            .collect();
    }
}

pub fn normalize_macro_key(key: &str) -> Result<String, MacroEngineError> {
    let key = key.trim().trim_start_matches(':').trim();
    if key.is_empty() {
        return Err(MacroEngineError::InvalidMacroKey);
    }
    Ok(key.to_ascii_lowercase())
}

fn mapping_touch(mapping: &MacroMapping) -> Option<(usize, ParamTarget)> {
    (!mapping.suspended && mapping.resolved_key.is_some())
        .then(|| (mapping.track, mapping.target.clone()))
}

pub fn lerp_curved(range_min: f32, range_max: f32, value: f32, curve: MacroCurve) -> f32 {
    let t = value.clamp(0.0, 1.0);
    let curved = match curve {
        MacroCurve::Linear => t,
        MacroCurve::Exp => t * t,
        MacroCurve::Log => t.sqrt(),
    };
    range_min + (range_max - range_min) * curved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect_target(logical_id: u64) -> ParamTarget {
        ParamTarget::EffectParam {
            slot: 0,
            effect: "filter".to_string(),
            param: "cutoff".to_string(),
            param_id: Some(ParamNodeId {
                logical_id,
                node_param_idx: 7,
            }),
        }
    }

    fn effect_key(logical_id: u64) -> MacroParamKey {
        MacroParamKey::Node {
            track: 0,
            param_id: ParamNodeId {
                logical_id,
                node_param_idx: 7,
            },
        }
    }

    fn mapped_engine(range_min: f32, range_max: f32) -> (MacroEngine, MacroId) {
        let mut engine = MacroEngine::default();
        let id = engine
            .create_macro("push", MacroKind::Mapped)
            .expect("macro id");
        engine
            .add_mapping(
                id,
                MacroMapping::new(
                    0,
                    effect_target(11),
                    range_min,
                    range_max,
                    MacroCurve::Linear,
                )
                .expect("mapping"),
            )
            .expect("known macro");
        (engine, id)
    }

    #[test]
    fn override_masks_base_and_release_restores_it() {
        let (mut engine, id) = mapped_engine(0.2, 0.8);
        let key = effect_key(11);

        engine.set_value(id, 0.5);
        assert_eq!(engine.effective_value(&key, 0.1), 0.5);
        assert_eq!(engine.effective_value(&key, 0.35), 0.5);

        engine.release(id);
        assert_eq!(engine.effective_value(&key, 0.35), 0.35);
        assert_eq!(engine.override_value(&key), None);
    }

    #[test]
    fn zero_is_an_engaged_continuous_macro_minimum() {
        let (mut engine, id) = mapped_engine(0.2, 0.8);
        let key = effect_key(11);

        engine.set_value(id, 1.0);
        engine.set_value(id, 0.0);

        assert_eq!(engine.override_value(&key), Some(0.2));
        assert!(engine.is_engaged(id));
    }

    #[test]
    fn one_parameter_can_only_be_owned_by_one_macro() {
        let (mut engine, first) = mapped_engine(0.0, 0.4);
        let second = engine
            .create_macro("second", MacroKind::Mapped)
            .expect("macro id");
        assert_eq!(
            engine.add_mapping(
                second,
                MacroMapping::new(0, effect_target(11), 0.0, 0.9, MacroCurve::Linear)
                    .expect("mapping"),
            ),
            Err(MacroEngineError::TargetAlreadyMapped { owner_id: first })
        );
        let key = effect_key(11);

        engine.set_value(first, 1.0);
        engine.set_value(second, 1.0);
        assert_eq!(engine.override_value(&key), Some(0.4));
        engine.release(first);
        assert_eq!(engine.override_value(&key), None);
        assert!(engine.macro_definition(second).unwrap().mappings.is_empty());
    }

    #[test]
    fn scene_revalidation_suspends_stale_mapping_and_reasserts_engaged_override() {
        let (mut engine, id) = mapped_engine(0.1, 0.8);
        let old_key = effect_key(11);
        let new_key = effect_key(22);
        engine.set_value(id, 1.0);

        engine.revalidate_mappings(|_, _| None);
        assert_eq!(engine.override_value(&old_key), None);
        assert!(engine.macro_definition(id).unwrap().mappings[0].suspended);

        engine.revalidate_mappings(|_, _| {
            Some(ResolvedMacroTarget {
                target: effect_target(22),
                key: new_key.clone(),
            })
        });
        assert_eq!(engine.override_value(&new_key), Some(0.8));
        assert!(!engine.macro_definition(id).unwrap().mappings[0].suspended);
    }

    #[test]
    fn curve_math_preserves_endpoints_and_shapes_midpoint() {
        for curve in [MacroCurve::Linear, MacroCurve::Exp, MacroCurve::Log] {
            assert_eq!(lerp_curved(10.0, 20.0, 0.0, curve), 10.0);
            assert_eq!(lerp_curved(10.0, 20.0, 1.0, curve), 20.0);
        }
        assert_eq!(lerp_curved(0.0, 1.0, 0.5, MacroCurve::Linear), 0.5);
        assert_eq!(lerp_curved(0.0, 1.0, 0.5, MacroCurve::Exp), 0.25);
        assert!((lerp_curved(0.0, 1.0, 0.25, MacroCurve::Log) - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn step_params_and_process_inlets_are_rejected() {
        assert_eq!(
            MacroMapping::new(
                0,
                ParamTarget::StepParam {
                    param: "velocity".to_string(),
                },
                0.0,
                1.0,
                MacroCurve::Linear,
            ),
            Err(MacroEngineError::UnsupportedTarget)
        );
        assert_eq!(
            MacroMapping::new(
                0,
                ParamTarget::ProcessInlet {
                    process: "foo".to_string(),
                    inlet: "bar".to_string(),
                    instance_id: None,
                },
                0.0,
                1.0,
                MacroCurve::Linear,
            ),
            Err(MacroEngineError::UnsupportedTarget)
        );
    }

    #[test]
    fn range_and_curve_edits_recompute_an_engaged_mapping() {
        let (mut engine, id) = mapped_engine(0.0, 1.0);
        let key = effect_key(11);
        engine.set_value(id, 0.5);

        assert_eq!(
            engine.set_mapping_range(id, 0, 10.0, 20.0),
            Ok(vec![(0, effect_target(11))])
        );
        assert_eq!(engine.override_value(&key), Some(15.0));

        engine
            .set_mapping_curve(id, 0, MacroCurve::Exp)
            .expect("mapping");
        assert_eq!(engine.override_value(&key), Some(12.5));
    }

    #[test]
    fn unmap_releases_override() {
        let (mut engine, id) = mapped_engine(0.0, 1.0);
        let key = effect_key(11);
        engine.set_value(id, 1.0);

        assert_eq!(
            engine.remove_mapping(id, 0),
            Ok(vec![(0, effect_target(11))])
        );
        assert_eq!(engine.override_value(&key), None);
        assert!(engine.macro_definition(id).unwrap().mappings.is_empty());
    }

    #[test]
    fn delete_releases_its_overrides_without_reusing_ids() {
        let (mut engine, first) = mapped_engine(0.0, 1.0);
        engine.set_value(first, 1.0);
        engine.delete_macro(first).expect("known macro");
        assert_eq!(engine.override_value(&effect_key(11)), None);

        let second = engine
            .create_macro("second", MacroKind::Mapped)
            .expect("next id");
        assert_eq!(second, first + 1);
    }

    #[test]
    fn keyed_ensure_is_idempotent_and_preserves_user_edits() {
        let mut engine = MacroEngine::default();
        let id = engine
            .ensure_macro(":Performance/Delay-Push", "Delay Push")
            .expect("ensure macro");
        engine.rename_macro(id, "My Delay").expect("rename");
        engine
            .add_mapping(
                id,
                MacroMapping::new(0, effect_target(11), 0.2, 0.8, MacroCurve::Log)
                    .expect("mapping"),
            )
            .expect("known macro");
        engine.set_value(id, 0.75);

        let ensured = engine
            .ensure_macro("performance/delay-push", "Reset Name")
            .expect("ensure existing");
        assert_eq!(ensured, id);
        assert_eq!(engine.macros().len(), 1);
        let macro_definition = engine.macro_definition(id).unwrap();
        assert_eq!(
            macro_definition.key.as_deref(),
            Some("performance/delay-push")
        );
        assert_eq!(macro_definition.name, "My Delay");
        assert_eq!(macro_definition.value, 0.75);
        assert_eq!(macro_definition.mappings.len(), 1);
        assert_eq!(macro_definition.mappings[0].range_min, 0.2);
        assert_eq!(macro_definition.mappings[0].curve, MacroCurve::Log);
    }

    #[test]
    fn duplicate_persisted_keys_are_rejected() {
        let mut engine = MacroEngine::default();
        let first = engine
            .ensure_macro("player/filter", "Filter")
            .expect("first macro");
        let mut duplicate = Macro::new(first + 1, "Duplicate", MacroKind::Mapped);
        duplicate.key = Some(":PLAYER/FILTER".to_string());

        assert_eq!(
            engine.insert_macro(duplicate),
            Err(MacroEngineError::DuplicateMacroKey(
                "player/filter".to_string()
            ))
        );
    }

    #[test]
    fn delete_then_ensure_same_key_allocates_a_fresh_id() {
        let mut engine = MacroEngine::default();
        let first = engine.ensure_macro("player/push", "Push").unwrap();
        engine.delete_macro(first).unwrap();
        let replacement = engine.ensure_macro("player/push", "Push").unwrap();

        assert_eq!(replacement, first + 1);
        assert_eq!(engine.macro_by_key(":PLAYER/PUSH").unwrap().id, replacement);
    }

    #[test]
    fn empty_script_key_is_rejected() {
        let mut engine = MacroEngine::default();
        assert_eq!(
            engine.ensure_macro(" : ", "Invalid"),
            Err(MacroEngineError::InvalidMacroKey)
        );
    }
}
