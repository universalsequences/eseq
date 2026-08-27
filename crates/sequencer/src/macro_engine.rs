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
use crate::sequencer::{remap_scene_index_after_move, BusId};

pub type MacroId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamScope {
    Track(usize),
    Bus(BusId),
}

impl From<usize> for ParamScope {
    fn from(track: usize) -> Self {
        Self::Track(track)
    }
}

impl From<BusId> for ParamScope {
    fn from(bus: BusId) -> Self {
        Self::Bus(bus)
    }
}

pub type ScopedParamTarget = (ParamScope, ParamTarget);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
    /// Interpolate positive stored values in descriptor/log space. Scene
    /// macros synthesize this curve for exponential parameters.
    LogDomain,
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
    pub scope: ParamScope,
    pub target: ParamTarget,
    pub range_min: f32,
    pub range_max: f32,
    pub curve: MacroCurve,
    /// Suspended mappings remain part of the macro but do not own an override.
    pub suspended: bool,
    resolved_key: Option<MacroParamKey>,
}

#[derive(Clone, Debug)]
pub struct TrackInstrumentMacroMappings {
    pub mappings: Vec<(MacroId, Vec<(usize, MacroMapping)>)>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackEffectMacroMappings {
    pub mappings: Vec<(MacroId, Vec<(usize, MacroMapping)>)>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackMidiFxMacroMappings {
    pub mappings: Vec<(MacroId, Vec<(usize, MacroMapping)>)>,
}

impl MacroMapping {
    pub fn new(
        scope: impl Into<ParamScope>,
        target: ParamTarget,
        range_min: f32,
        range_max: f32,
        curve: MacroCurve,
    ) -> Result<Self, MacroEngineError> {
        Self::new_resolved(scope, target, None, range_min, range_max, curve)
    }

    /// Constructs a mapping whose descriptor index has already been resolved.
    /// The index is used only when the live node has no stable `ParamNodeId`.
    pub fn new_resolved(
        scope: impl Into<ParamScope>,
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

        let scope = scope.into();
        let resolved_key = MacroParamKey::from_target(scope, &target, param_idx);
        Ok(Self {
            scope,
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
    RackMacro {
        track: usize,
        macro_id: u8,
    },
    BusNode {
        bus: BusId,
        param_id: ParamNodeId,
    },
    BusEffect {
        bus: BusId,
        slot: usize,
        param: usize,
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

    pub fn for_rack_macro(track: usize, macro_id: u8) -> Self {
        Self::RackMacro { track, macro_id }
    }

    pub fn for_bus_effect(
        bus: BusId,
        slot: usize,
        param_idx: usize,
        param_id: Option<ParamNodeId>,
    ) -> Self {
        param_id.map_or(
            Self::BusEffect {
                bus,
                slot,
                param: param_idx,
            },
            |param_id| Self::BusNode { bus, param_id },
        )
    }

    pub fn from_target(
        scope: ParamScope,
        target: &ParamTarget,
        param_idx: Option<usize>,
    ) -> Option<Self> {
        let ParamScope::Track(track) = scope else {
            let bus = match scope {
                ParamScope::Bus(bus) => bus,
                ParamScope::Track(_) => unreachable!(),
            };
            return match target {
                ParamTarget::EffectParam { slot, param_id, .. } => param_id
                    .map(|param_id| Self::BusNode { bus, param_id })
                    .or_else(|| {
                        param_idx.map(|param| Self::BusEffect {
                            bus,
                            slot: *slot,
                            param,
                        })
                    }),
                _ => None,
            };
        };
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
            ParamTarget::RackMacroParam { macro_id } => {
                Some(Self::for_rack_macro(track, *macro_id))
            }
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
    NotSceneMacro(MacroId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMacroTarget {
    pub target: ParamTarget,
    pub key: MacroParamKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverrideOwner {
    Macro(MacroId),
    ScenePush,
}

#[derive(Clone, Copy, Debug)]
struct ActiveOverride {
    owner: OverrideOwner,
    value: f32,
    write_order: u64,
}

#[derive(Debug)]
struct ScenePushOverride {
    mappings: Vec<MacroMapping>,
    value: f32,
    write_order: u64,
}

#[derive(Debug)]
pub struct MacroEngine {
    macros: Vec<Macro>,
    next_id: MacroId,
    overrides: HashMap<MacroParamKey, f32>,
    next_write_order: u64,
    scene_push: Option<ScenePushOverride>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroConfigurationState {
    pub macros: Vec<Macro>,
    pub next_id: MacroId,
}

#[derive(Clone, Debug)]
pub struct TrackTopologyMacroMappings {
    pub first_track: usize,
    pub mappings: Vec<(MacroId, Vec<(usize, MacroMapping)>)>,
}

impl Default for MacroEngine {
    fn default() -> Self {
        Self {
            macros: Vec::new(),
            next_id: 1,
            overrides: HashMap::new(),
            next_write_order: 1,
            scene_push: None,
        }
    }
}

impl MacroEngine {
    pub fn capture_configuration(&self) -> MacroConfigurationState {
        MacroConfigurationState {
            macros: self.macros.clone(),
            next_id: self.next_id,
        }
    }

    pub fn restore_configuration(&mut self, target: &MacroConfigurationState) {
        let live = self.macros.iter().map(|definition| {
            (definition.id, (definition.value, definition.last_write_order))
        }).collect::<HashMap<_, _>>();
        let allocation_floor = self.next_id;
        self.macros = target.macros.clone();
        for definition in &mut self.macros {
            if let Some((value, write_order)) = live.get(&definition.id) {
                definition.value = *value;
                definition.last_write_order = *write_order;
            }
        }
        self.next_id = allocation_floor.max(target.next_id);
        self.rebuild_ownership();
    }

    pub fn capture_track_topology_mappings(
        &self,
        first_track: usize,
    ) -> TrackTopologyMacroMappings {
        TrackTopologyMacroMappings {
            first_track,
            mappings: self.macros.iter().filter_map(|definition| {
                let mappings = definition.mappings.iter().enumerate()
                    .filter(|(_, mapping)| matches!(mapping.scope, ParamScope::Track(track) if track >= first_track))
                    .map(|(index, mapping)| (index, mapping.clone()))
                    .collect::<Vec<_>>();
                (!mappings.is_empty()).then_some((definition.id, mappings))
            }).collect(),
        }
    }

    pub fn remap_after_track_delete(&mut self, deleted: usize) {
        for definition in &mut self.macros {
            definition.mappings.retain_mut(|mapping| match &mut mapping.scope {
                ParamScope::Track(track) if *track == deleted => false,
                ParamScope::Track(track) if *track > deleted => {
                    *track -= 1;
                    true
                }
                ParamScope::Track(_) | ParamScope::Bus(_) => true,
            });
        }
        self.rebuild_ownership();
    }

    pub fn restore_track_topology_mappings(
        &mut self,
        snapshot: &TrackTopologyMacroMappings,
    ) -> Result<(), MacroEngineError> {
        for definition in &mut self.macros {
            definition.mappings.retain(|mapping| {
                !matches!(mapping.scope, ParamScope::Track(track) if track >= snapshot.first_track)
            });
        }
        for (macro_id, mappings) in &snapshot.mappings {
            let definition = self.macros.iter_mut()
                .find(|definition| definition.id == *macro_id)
                .ok_or(MacroEngineError::UnknownMacro(*macro_id))?;
            for (index, mapping) in mappings {
                definition.mappings.insert((*index).min(definition.mappings.len()), mapping.clone());
            }
        }
        self.rebuild_ownership();
        Ok(())
    }

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

    pub fn set_scene_config(
        &mut self,
        id: MacroId,
        config: SceneMacroConfig,
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
        let Some(definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        if !matches!(definition.kind, MacroKind::Scene(_)) {
            return Err(MacroEngineError::NotSceneMacro(id));
        }
        let touched = definition
            .mappings
            .iter()
            .filter_map(mapping_touch)
            .collect();
        definition.kind = MacroKind::Scene(config);
        definition.mappings.clear();
        definition.value = 0.0;
        definition.last_write_order = None;
        self.rebuild_ownership();
        Ok(touched)
    }

    /// Installs the transient diff synthesized at scene-macro engagement.
    /// Targets already owned by another macro are skipped as specified by the
    /// later-writer scene-macro rule; these mappings are never persisted.
    pub fn engage_scene(
        &mut self,
        id: MacroId,
        mappings: Vec<MacroMapping>,
        value: f32,
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
        let Some(index) = self.macros.iter().position(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        if !matches!(self.macros[index].kind, MacroKind::Scene(_)) {
            return Err(MacroEngineError::NotSceneMacro(id));
        }
        let accepted = mappings
            .into_iter()
            .filter(|mapping| {
                mapping
                    .resolved_key
                    .as_ref()
                    .is_none_or(|key| self.mapping_owner(key).is_none_or(|owner| owner == id))
            })
            .collect::<Vec<_>>();
        let previous = std::mem::take(&mut self.macros[index].mappings);
        let mut touched = previous
            .into_iter()
            .filter_map(|mapping| mapping_touch(&mapping))
            .collect::<Vec<_>>();
        touched.extend(accepted.iter().filter_map(mapping_touch));
        let write_order = self.allocate_write_order();
        self.macros[index].mappings = accepted;
        self.macros[index].value = value.clamp(0.0, 1.0);
        self.macros[index].last_write_order = Some(write_order);
        self.rebuild_ownership();
        Ok(touched)
    }

    pub fn scene_config(&self, id: MacroId) -> Option<&SceneMacroConfig> {
        match &self.macro_definition(id)?.kind {
            MacroKind::Scene(config) => Some(config),
            MacroKind::Mapped => None,
        }
    }

    /// Deletes a macro and returns its targets so the App can re-send whichever
    /// owner (or base value) is revealed by the deletion.
    pub fn delete_macro(
        &mut self,
        id: MacroId,
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
        let Some(index) = self.macros.iter().position(|item| item.id == id) else {
            return Err(MacroEngineError::UnknownMacro(id));
        };
        let removed = self.macros.remove(index);
        let touched = removed
            .mappings
            .into_iter()
            .filter(|mapping| !mapping.suspended && mapping.resolved_key.is_some())
            .map(|mapping| (mapping.scope, mapping.target))
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
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
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
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
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
    ) -> Result<Vec<ScopedParamTarget>, MacroEngineError> {
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

    /// Removes mappings that belong to the instrument currently occupying a
    /// custom track. Track effects are separate devices and deliberately keep
    /// their mappings across an instrument swap.
    ///
    /// Suspended mappings are removed too: although they do not own a live
    /// override, retaining them would allow a later scene revalidation to bind
    /// an old instrument mapping to the replacement instrument by name.
    pub fn remove_instrument_mappings_for_track(&mut self, track: usize) -> usize {
        let mut removed = 0;
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain(|mapping| {
                let should_remove = mapping.scope == ParamScope::Track(track)
                    && matches!(&mapping.target, ParamTarget::InstrumentParam { .. });
                removed += usize::from(should_remove);
                !should_remove
            });
        }
        if removed > 0 {
            self.rebuild_ownership();
        }
        removed
    }

    pub fn capture_instrument_mappings_for_track(
        &self,
        track: usize,
    ) -> TrackInstrumentMacroMappings {
        TrackInstrumentMacroMappings {
            mappings: self
                .macros
                .iter()
                .filter_map(|macro_definition| {
                    let mappings = macro_definition
                        .mappings
                        .iter()
                        .enumerate()
                        .filter(|(_, mapping)| {
                            mapping.scope == ParamScope::Track(track)
                                && matches!(
                                    mapping.target,
                                    ParamTarget::InstrumentParam { .. }
                                )
                        })
                        .map(|(index, mapping)| (index, mapping.clone()))
                        .collect::<Vec<_>>();
                    (!mappings.is_empty()).then_some((macro_definition.id, mappings))
                })
                .collect(),
        }
    }

    pub fn restore_instrument_mappings_for_track(
        &mut self,
        track: usize,
        snapshot: &TrackInstrumentMacroMappings,
    ) -> Result<(), MacroEngineError> {
        self.remove_instrument_mappings_for_track(track);
        for (macro_id, mappings) in &snapshot.mappings {
            let macro_definition = self
                .macros
                .iter_mut()
                .find(|definition| definition.id == *macro_id)
                .ok_or(MacroEngineError::UnknownMacro(*macro_id))?;
            for (index, mapping) in mappings {
                macro_definition
                    .mappings
                    .insert((*index).min(macro_definition.mappings.len()), mapping.clone());
            }
        }
        self.rebuild_ownership();
        Ok(())
    }

    pub fn capture_effect_mappings_for_track(&self, track: usize) -> TrackEffectMacroMappings {
        TrackEffectMacroMappings {
            mappings: self
                .macros
                .iter()
                .filter_map(|macro_definition| {
                    let mappings = macro_definition
                        .mappings
                        .iter()
                        .enumerate()
                        .filter(|(_, mapping)| {
                            mapping.scope == ParamScope::Track(track)
                                && matches!(mapping.target, ParamTarget::EffectParam { .. })
                        })
                        .map(|(index, mapping)| (index, mapping.clone()))
                        .collect::<Vec<_>>();
                    (!mappings.is_empty()).then_some((macro_definition.id, mappings))
                })
                .collect(),
        }
    }

    pub fn restore_effect_mappings_for_track(
        &mut self,
        track: usize,
        snapshot: &TrackEffectMacroMappings,
    ) -> Result<(), MacroEngineError> {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain(|mapping| {
                mapping.scope != ParamScope::Track(track)
                    || !matches!(mapping.target, ParamTarget::EffectParam { .. })
            });
        }
        for (macro_id, mappings) in &snapshot.mappings {
            let macro_definition = self
                .macros
                .iter_mut()
                .find(|definition| definition.id == *macro_id)
                .ok_or(MacroEngineError::UnknownMacro(*macro_id))?;
            for (index, mapping) in mappings {
                macro_definition
                    .mappings
                    .insert((*index).min(macro_definition.mappings.len()), mapping.clone());
            }
        }
        self.rebuild_ownership();
        Ok(())
    }

    pub fn remap_effect_mappings_for_track(
        &mut self,
        track: usize,
        old_to_new: &[Option<usize>],
    ) {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain_mut(|mapping| {
                if mapping.scope != ParamScope::Track(track) {
                    return true;
                }
                let ParamTarget::EffectParam { slot, .. } = &mut mapping.target else {
                    return true;
                };
                let Some(new_slot) = old_to_new.get(*slot).copied().flatten() else {
                    return false;
                };
                *slot = new_slot;
                true
            });
        }
        self.rebuild_ownership();
    }

    pub fn capture_effect_mappings_for_bus(&self, bus: BusId) -> TrackEffectMacroMappings {
        TrackEffectMacroMappings {
            mappings: self
                .macros
                .iter()
                .filter_map(|macro_definition| {
                    let mappings = macro_definition
                        .mappings
                        .iter()
                        .enumerate()
                        .filter(|(_, mapping)| {
                            mapping.scope == ParamScope::Bus(bus)
                                && matches!(mapping.target, ParamTarget::EffectParam { .. })
                        })
                        .map(|(index, mapping)| (index, mapping.clone()))
                        .collect::<Vec<_>>();
                    (!mappings.is_empty()).then_some((macro_definition.id, mappings))
                })
                .collect(),
        }
    }

    pub fn restore_effect_mappings_for_bus(
        &mut self,
        bus: BusId,
        snapshot: &TrackEffectMacroMappings,
    ) -> Result<(), MacroEngineError> {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain(|mapping| {
                mapping.scope != ParamScope::Bus(bus)
                    || !matches!(mapping.target, ParamTarget::EffectParam { .. })
            });
        }
        for (macro_id, mappings) in &snapshot.mappings {
            let macro_definition = self
                .macros
                .iter_mut()
                .find(|definition| definition.id == *macro_id)
                .ok_or(MacroEngineError::UnknownMacro(*macro_id))?;
            for (index, mapping) in mappings {
                macro_definition
                    .mappings
                    .insert((*index).min(macro_definition.mappings.len()), mapping.clone());
            }
        }
        self.rebuild_ownership();
        Ok(())
    }

    pub fn remap_effect_mappings_for_bus(
        &mut self,
        bus: BusId,
        old_to_new: &[Option<usize>],
    ) {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain_mut(|mapping| {
                if mapping.scope != ParamScope::Bus(bus) {
                    return true;
                }
                let ParamTarget::EffectParam { slot, .. } = &mut mapping.target else {
                    return true;
                };
                let Some(new_slot) = old_to_new.get(*slot).copied().flatten() else {
                    return false;
                };
                *slot = new_slot;
                true
            });
        }
        self.rebuild_ownership();
    }

    pub fn capture_midi_fx_mappings_for_track(&self, track: usize) -> TrackMidiFxMacroMappings {
        TrackMidiFxMacroMappings {
            mappings: self
                .macros
                .iter()
                .filter_map(|macro_definition| {
                    let mappings = macro_definition
                        .mappings
                        .iter()
                        .enumerate()
                        .filter(|(_, mapping)| {
                            mapping.scope == ParamScope::Track(track)
                                && matches!(mapping.target, ParamTarget::MidiFxParam { .. })
                        })
                        .map(|(index, mapping)| (index, mapping.clone()))
                        .collect::<Vec<_>>();
                    (!mappings.is_empty()).then_some((macro_definition.id, mappings))
                })
                .collect(),
        }
    }

    pub fn restore_midi_fx_mappings_for_track(
        &mut self,
        track: usize,
        snapshot: &TrackMidiFxMacroMappings,
    ) -> Result<(), MacroEngineError> {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain(|mapping| {
                mapping.scope != ParamScope::Track(track)
                    || !matches!(mapping.target, ParamTarget::MidiFxParam { .. })
            });
        }
        for (macro_id, mappings) in &snapshot.mappings {
            let macro_definition = self
                .macros
                .iter_mut()
                .find(|definition| definition.id == *macro_id)
                .ok_or(MacroEngineError::UnknownMacro(*macro_id))?;
            for (index, mapping) in mappings {
                macro_definition
                    .mappings
                    .insert((*index).min(macro_definition.mappings.len()), mapping.clone());
            }
        }
        self.rebuild_ownership();
        Ok(())
    }

    pub fn remap_midi_fx_mappings_for_track(
        &mut self,
        track: usize,
        old_to_new: &[Option<usize>],
    ) {
        for macro_definition in &mut self.macros {
            macro_definition.mappings.retain_mut(|mapping| {
                if mapping.scope != ParamScope::Track(track) {
                    return true;
                }
                let ParamTarget::MidiFxParam { slot, .. } = &mut mapping.target else {
                    return true;
                };
                let Some(new_slot) = old_to_new.get(*slot).copied().flatten() else {
                    return false;
                };
                *slot = new_slot;
                true
            });
        }
        self.rebuild_ownership();
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
    pub fn set_value(&mut self, id: MacroId, value: f32) -> Vec<ScopedParamTarget> {
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
                .map(|mapping| (mapping.scope, mapping.target.clone()))
                .collect()
        };
        self.rebuild_ownership();
        touched
    }

    /// Explicitly releases a momentary macro and returns its visible position
    /// to zero. This remains distinct from `set_value(id, 0.0)`, which engages
    /// a continuous macro at its mapped minimum.
    pub fn release(&mut self, id: MacroId) -> Vec<ScopedParamTarget> {
        let Some(macro_definition) = self.macros.iter_mut().find(|item| item.id == id) else {
            return Vec::new();
        };
        macro_definition.value = 0.0;
        macro_definition.last_write_order = None;
        let touched = macro_definition
            .mappings
            .iter()
            .filter(|mapping| !mapping.suspended && mapping.resolved_key.is_some())
            .map(|mapping| (mapping.scope, mapping.target.clone()))
            .collect();
        if matches!(macro_definition.kind, MacroKind::Scene(_)) {
            macro_definition.mappings.clear();
        }
        self.rebuild_ownership();
        touched
    }

    pub fn release_all_scene_macros(&mut self) -> Vec<ScopedParamTarget> {
        let ids = self
            .macros
            .iter()
            .filter(|definition| matches!(definition.kind, MacroKind::Scene(_)))
            .map(|definition| definition.id)
            .collect::<Vec<_>>();
        ids.into_iter().flat_map(|id| self.release(id)).collect()
    }

    /// Starts a non-persistent scene morph owned by the current pointer
    /// gesture. Persistent macro ownership is respected, so a performance
    /// gesture cannot silently steal a mapped control from the project.
    pub fn begin_scene_push(
        &mut self,
        mappings: Vec<MacroMapping>,
        value: f32,
    ) -> Vec<ScopedParamTarget> {
        let mut touched = self.end_scene_push();
        let accepted = mappings
            .into_iter()
            .filter(|mapping| {
                mapping
                    .resolved_key
                    .as_ref()
                    .is_none_or(|key| self.mapping_owner(key).is_none())
            })
            .collect::<Vec<_>>();
        touched.extend(accepted.iter().filter_map(mapping_touch));
        let write_order = self.allocate_write_order();
        self.scene_push = Some(ScenePushOverride {
            mappings: accepted,
            value: if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            },
            write_order,
        });
        self.rebuild_ownership();
        touched
    }

    pub fn set_scene_push_value(&mut self, value: f32) -> Vec<ScopedParamTarget> {
        if !value.is_finite() || self.scene_push.is_none() {
            return Vec::new();
        }
        let write_order = self.allocate_write_order();
        let scene_push = self.scene_push.as_mut().expect("checked above");
        scene_push.value = value.clamp(0.0, 1.0);
        scene_push.write_order = write_order;
        let touched = scene_push
            .mappings
            .iter()
            .filter_map(mapping_touch)
            .collect();
        self.rebuild_ownership();
        touched
    }

    pub fn end_scene_push(&mut self) -> Vec<ScopedParamTarget> {
        let Some(scene_push) = self.scene_push.take() else {
            return Vec::new();
        };
        let touched = scene_push
            .mappings
            .iter()
            .filter_map(mapping_touch)
            .collect();
        self.rebuild_ownership();
        touched
    }

    pub fn remap_scene_targets_after_move(&mut self, source: usize, target: usize) {
        for definition in &mut self.macros {
            let MacroKind::Scene(config) = &mut definition.kind else {
                continue;
            };
            config.target_scene =
                remap_scene_index_after_move(config.target_scene, source, target);
        }
    }

    pub fn remap_scene_targets_after_delete(&mut self, deleted: usize, scene_count: usize) {
        let last = scene_count.saturating_sub(1);
        for definition in &mut self.macros {
            let MacroKind::Scene(config) = &mut definition.kind else {
                continue;
            };
            config.target_scene = if config.target_scene > deleted {
                config.target_scene - 1
            } else if config.target_scene == deleted {
                deleted.min(last)
            } else {
                config.target_scene.min(last)
            };
        }
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
                        .map(|order| (OverrideOwner::Macro(macro_definition.id), index, order))
                })
                .collect::<Vec<_>>();
            if let Some(scene_push) = &self.scene_push {
                engaged.push((OverrideOwner::ScenePush, usize::MAX, scene_push.write_order));
            }
            engaged.sort_unstable_by_key(|(_, _, order)| *order);
            for (new_order, (owner, index, _)) in engaged.iter().enumerate() {
                let order = new_order as u64 + 1;
                match owner {
                    OverrideOwner::Macro(_) => self.macros[*index].last_write_order = Some(order),
                    OverrideOwner::ScenePush => {
                        self.scene_push.as_mut().expect("listed above").write_order = order
                    }
                }
            }
            self.next_write_order = engaged.len() as u64 + 1;
        }
        let order = self.next_write_order;
        self.next_write_order += 1;
        order
    }

    /// Re-resolves every mapping against the current scene. Missing targets are
    /// retained as suspended mappings, and engaged macros are rebuilt onto the
    /// newly resolved identities without changing their writer precedence.
    pub fn revalidate_mappings<F>(&mut self, mut resolve: F) -> Vec<ScopedParamTarget>
    where
        F: FnMut(ParamScope, &ParamTarget) -> Option<ResolvedMacroTarget>,
    {
        let mut touched = Vec::new();
        for macro_definition in &mut self.macros {
            for mapping in &mut macro_definition.mappings {
                touched.push((mapping.scope, mapping.target.clone()));
                if let Some(resolved) = resolve(mapping.scope, &mapping.target) {
                    mapping.target = resolved.target;
                    mapping.resolved_key = Some(resolved.key);
                    mapping.suspended = false;
                } else {
                    mapping.resolved_key = None;
                    mapping.suspended = true;
                }
                touched.push((mapping.scope, mapping.target.clone()));
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
                    .find(|entry| entry.owner == OverrideOwner::Macro(macro_definition.id))
                {
                    existing.value = value;
                    existing.write_order = write_order;
                } else {
                    entries.push(ActiveOverride {
                        owner: OverrideOwner::Macro(macro_definition.id),
                        value,
                        write_order,
                    });
                }
            }
        }

        if let Some(scene_push) = &self.scene_push {
            for mapping in &scene_push.mappings {
                if mapping.suspended {
                    continue;
                }
                let Some(key) = mapping.resolved_key.clone() else {
                    continue;
                };
                owners.entry(key).or_default().push(ActiveOverride {
                    owner: OverrideOwner::ScenePush,
                    value: lerp_curved(
                        mapping.range_min,
                        mapping.range_max,
                        scene_push.value,
                        mapping.curve,
                    ),
                    write_order: scene_push.write_order,
                });
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

fn mapping_touch(mapping: &MacroMapping) -> Option<ScopedParamTarget> {
    (!mapping.suspended && mapping.resolved_key.is_some())
        .then(|| (mapping.scope, mapping.target.clone()))
}

pub fn lerp_curved(range_min: f32, range_max: f32, value: f32, curve: MacroCurve) -> f32 {
    let t = value.clamp(0.0, 1.0);
    let curved = match curve {
        MacroCurve::Linear => t,
        MacroCurve::Exp => t * t,
        MacroCurve::Log => t.sqrt(),
        MacroCurve::LogDomain => {
            if range_min > 0.0 && range_max > 0.0 {
                return (range_min.ln() + (range_max.ln() - range_min.ln()) * t).exp();
            }
            t
        }
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

    #[test]
    fn scene_push_is_ephemeral_interpolated_and_restores_the_base_layer() {
        let mut engine = MacroEngine::default();
        let target = effect_target(91);
        let key = effect_key(91);
        let mapping = MacroMapping::new(0, target.clone(), 0.2, 0.8, MacroCurve::Linear).unwrap();

        assert_eq!(
            engine.begin_scene_push(vec![mapping], 1.0),
            vec![(ParamScope::Track(0), target.clone())]
        );
        assert_eq!(engine.override_value(&key), Some(0.8));

        assert_eq!(
            engine.set_scene_push_value(0.25),
            vec![(ParamScope::Track(0), target.clone())]
        );
        assert!((engine.override_value(&key).unwrap() - 0.35).abs() < 1.0e-6);

        assert_eq!(
            engine.end_scene_push(),
            vec![(ParamScope::Track(0), target)]
        );
        assert_eq!(engine.override_value(&key), None);
        assert!(
            engine.macros().is_empty(),
            "scene pushes must never enter project persistence"
        );
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
        assert_eq!(engine.macro_definition(id).unwrap().value, 0.0);
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
    fn log_domain_curve_interpolates_geometrically() {
        assert!((lerp_curved(20.0, 20_000.0, 0.5, MacroCurve::LogDomain) - 632.4555).abs() < 0.1);
    }

    #[test]
    fn scene_mappings_are_transient_and_discarded_on_release() {
        let mut engine = MacroEngine::default();
        let id = engine
            .create_macro(
                "scene push",
                MacroKind::Scene(SceneMacroConfig {
                    target_scene: 1,
                    morph_params: true,
                    steal_patterns: false,
                    quantize: StealQuantize::Bar,
                    track_mask: None,
                }),
            )
            .unwrap();
        let mapping = MacroMapping::new_resolved(
            0,
            effect_target(88),
            Some(0),
            20.0,
            20_000.0,
            MacroCurve::LogDomain,
        )
        .unwrap();
        let touched = engine.engage_scene(id, vec![mapping], 0.5).unwrap();
        assert_eq!(touched.len(), 1);
        assert_eq!(engine.macro_definition(id).unwrap().mappings.len(), 1);
        assert!((engine.override_value(&effect_key(88)).unwrap() - 632.4555).abs() < 0.1);

        engine.release(id);
        assert!(engine.macro_definition(id).unwrap().mappings.is_empty());
        assert_eq!(engine.override_value(&effect_key(88)), None);
    }

    #[test]
    fn scene_move_remaps_scene_macro_targets_by_identity() {
        let mut engine = MacroEngine::default();
        for target_scene in [0, 1, 2, 3] {
            engine
                .create_macro(
                    format!("scene {target_scene}"),
                    MacroKind::Scene(SceneMacroConfig {
                        target_scene,
                        morph_params: true,
                        steal_patterns: false,
                        quantize: StealQuantize::Bar,
                        track_mask: None,
                    }),
                )
                .unwrap();
        }
        engine.remap_scene_targets_after_move(0, 2);
        let targets = engine
            .macros()
            .iter()
            .map(|definition| match &definition.kind {
                MacroKind::Scene(config) => config.target_scene,
                MacroKind::Mapped => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(targets, vec![2, 0, 1, 3]);
    }

    #[test]
    fn scene_delete_remaps_and_clamps_scene_macro_targets() {
        let mut engine = MacroEngine::default();
        for target_scene in [0, 1, 3] {
            engine
                .create_macro(
                    format!("scene {target_scene}"),
                    MacroKind::Scene(SceneMacroConfig {
                        target_scene,
                        morph_params: true,
                        steal_patterns: false,
                        quantize: StealQuantize::Bar,
                        track_mask: None,
                    }),
                )
                .unwrap();
        }
        engine.remap_scene_targets_after_delete(1, 3);
        let targets = engine
            .macros()
            .iter()
            .map(|definition| match &definition.kind {
                MacroKind::Scene(config) => config.target_scene,
                MacroKind::Mapped => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(targets, vec![0, 1, 2]);
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
            Ok(vec![(ParamScope::Track(0), effect_target(11))])
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
            Ok(vec![(ParamScope::Track(0), effect_target(11))])
        );
        assert_eq!(engine.override_value(&key), None);
        assert!(engine.macro_definition(id).unwrap().mappings.is_empty());
    }

    #[test]
    fn instrument_swap_removes_only_that_tracks_instrument_mappings() {
        let mut engine = MacroEngine::default();
        let id = engine
            .create_macro("mixed targets", MacroKind::Mapped)
            .expect("macro id");
        let instrument_target = || ParamTarget::InstrumentParam {
            param: "tone".to_string(),
            param_id: None,
        };

        engine
            .add_mapping(
                id,
                MacroMapping::new_resolved(
                    0,
                    instrument_target(),
                    Some(0),
                    0.0,
                    1.0,
                    MacroCurve::Linear,
                )
                .expect("resolved instrument mapping"),
            )
            .expect("track 1 instrument mapping");
        engine
            .add_mapping(
                id,
                MacroMapping::new(0, instrument_target(), 0.0, 1.0, MacroCurve::Linear)
                    .expect("suspended instrument mapping"),
            )
            .expect("track 1 suspended instrument mapping");
        engine
            .add_mapping(
                id,
                MacroMapping::new(0, effect_target(11), 0.2, 0.8, MacroCurve::Linear)
                    .expect("effect mapping"),
            )
            .expect("track 1 effect mapping");
        engine
            .add_mapping(
                id,
                MacroMapping::new_resolved(
                    1,
                    instrument_target(),
                    Some(0),
                    0.1,
                    0.9,
                    MacroCurve::Linear,
                )
                .expect("other track instrument mapping"),
            )
            .expect("track 2 instrument mapping");
        engine.set_value(id, 0.5);

        assert_eq!(engine.remove_instrument_mappings_for_track(0), 2);

        let mappings = &engine.macro_definition(id).expect("macro remains").mappings;
        assert_eq!(mappings.len(), 2);
        assert!(mappings.iter().any(|mapping| {
            mapping.scope == ParamScope::Track(0)
                && matches!(&mapping.target, ParamTarget::EffectParam { .. })
        }));
        assert!(mappings.iter().any(|mapping| {
            mapping.scope == ParamScope::Track(1)
                && matches!(&mapping.target, ParamTarget::InstrumentParam { .. })
        }));
        assert_eq!(
            engine.override_value(&MacroParamKey::Instrument { track: 0, param: 0 }),
            None
        );
        assert!(engine
            .override_value(&effect_key(11))
            .is_some_and(|value| (value - 0.5).abs() < 1.0e-6));
        assert!(engine
            .override_value(&MacroParamKey::Instrument { track: 1, param: 0 })
            .is_some_and(|value| (value - 0.5).abs() < 1.0e-6));
    }

    #[test]
    fn effect_chain_remap_moves_targets_and_drops_deleted_instances() {
        let mut engine = MacroEngine::default();
        let id = engine.create_macro("fx", MacroKind::Mapped).unwrap();
        let mut moved = effect_target(11);
        if let ParamTarget::EffectParam { slot, .. } = &mut moved {
            *slot = 4;
        }
        let mut deleted = effect_target(12);
        if let ParamTarget::EffectParam { slot, .. } = &mut deleted {
            *slot = 5;
        }
        engine
            .add_mapping(
                id,
                MacroMapping::new(0, moved, 0.0, 1.0, MacroCurve::Linear).unwrap(),
            )
            .unwrap();
        engine
            .add_mapping(
                id,
                MacroMapping::new(0, deleted, 0.0, 1.0, MacroCurve::Linear).unwrap(),
            )
            .unwrap();
        let mut old_to_new = (0..8).map(Some).collect::<Vec<_>>();
        old_to_new[4] = Some(6);
        old_to_new[5] = None;

        engine.remap_effect_mappings_for_track(0, &old_to_new);

        let mappings = &engine.macro_definition(id).unwrap().mappings;
        assert_eq!(mappings.len(), 1);
        assert!(matches!(
            mappings[0].target,
            ParamTarget::EffectParam { slot: 6, .. }
        ));
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
    fn track_topology_mapping_snapshot_restores_deleted_and_shifted_scopes() {
        let mut engine = MacroEngine::default();
        let id = engine.create_macro("Topology", MacroKind::Mapped).unwrap();
        for track in 0..3 {
            engine.add_mapping(id, MacroMapping::new(
                track,
                ParamTarget::RackMacroParam { macro_id: 0 },
                0.0,
                1.0,
                MacroCurve::Linear,
            ).unwrap()).unwrap();
        }
        let snapshot = engine.capture_track_topology_mappings(1);

        engine.remap_after_track_delete(1);
        assert_eq!(
            engine.macro_definition(id).unwrap().mappings.iter()
                .filter_map(|mapping| match mapping.scope {
                    ParamScope::Track(track) => Some(track),
                    ParamScope::Bus(_) => None,
                }).collect::<Vec<_>>(),
            vec![0, 1],
        );

        engine.restore_track_topology_mappings(&snapshot).unwrap();
        assert_eq!(
            engine.macro_definition(id).unwrap().mappings.iter()
                .filter_map(|mapping| match mapping.scope {
                    ParamScope::Track(track) => Some(track),
                    ParamScope::Bus(_) => None,
                }).collect::<Vec<_>>(),
            vec![0, 1, 2],
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

    #[test]
    fn track_instrument_mapping_snapshot_restores_order_without_touching_other_targets() {
        let mut engine = MacroEngine::default();
        let id = engine.create_macro("instrument history", MacroKind::Mapped).unwrap();
        let track_zero = MacroMapping::new_resolved(
            0,
            ParamTarget::InstrumentParam {
                param: "tone".to_string(),
                param_id: None,
            },
            Some(0),
            0.1,
            0.9,
            MacroCurve::Linear,
        )
        .unwrap();
        let effect = MacroMapping::new_resolved(
            0,
            effect_target(11),
            Some(0),
            0.0,
            1.0,
            MacroCurve::Linear,
        )
        .unwrap();
        let track_one = MacroMapping::new_resolved(
            1,
            ParamTarget::InstrumentParam {
                param: "tone".to_string(),
                param_id: None,
            },
            Some(0),
            0.2,
            0.8,
            MacroCurve::Linear,
        )
        .unwrap();
        engine.add_mapping(id, track_zero.clone()).unwrap();
        engine.add_mapping(id, effect.clone()).unwrap();
        engine.add_mapping(id, track_one.clone()).unwrap();

        let snapshot = engine.capture_instrument_mappings_for_track(0);
        assert_eq!(engine.remove_instrument_mappings_for_track(0), 1);
        engine
            .restore_instrument_mappings_for_track(0, &snapshot)
            .unwrap();

        let mappings = &engine.macro_definition(id).unwrap().mappings;
        assert_eq!(mappings, &[track_zero, effect, track_one]);
    }
}
