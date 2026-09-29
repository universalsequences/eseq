//! Promote user content into the shipped factory tier (eseq-jhmx).
//!
//! A Sound, kit or rack preset saved on one machine usually leans on things
//! that only exist there: user-tier instruments, custom effects, imported
//! samples, user filter tables. Shipping it as-is gives a new machine a Sound
//! that fails to load. Promotion copies the object into `content/` and, on
//! the way, drops every dependency that would not be there on a fresh
//! install — a rack slot, a pad, an insert, a sequencer — reporting each one,
//! so what lands in the factory tier loads the same everywhere.
//!
//! "Factory" is decided by where a reference resolves on disk: a dependency
//! is factory when it resolves under [`AppPaths::factory_root`]. That one rule
//! covers the factory instrument/effect trees, shipped packages, and the
//! bundled filter tables, and correctly rejects `factory:` ids that only
//! still resolve through the user tier or the dev fixture tree. Samples are
//! factory when their content hash is in a shipped package's `samples.jsonl`.
//!
//! The pure half ([`sanitize_sound`], [`sanitize_kit`]) takes a
//! [`FactoryIndex`] so tests can drive it without a checkout; the app half
//! lives in `app::factory_promote`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::app_paths::AppPaths;
use crate::project::{
    ProjectEffectSlot, ProjectInstrumentType, ProjectKitModDestination, ProjectKitPreset,
    ProjectRackMacroTarget, ProjectRackTrackPattern, ProjectSoundPreset, ProjectTrackKind,
};

#[cfg(test)]
mod tests;

/// Answers "does this reference exist on a fresh install?".
pub trait FactoryIndex {
    fn instrument(&self, id: &str) -> bool;
    fn effect(&self, name: &str) -> bool;
    /// The portable `samples/<hash>.wav` spelling of a factory sample, or
    /// `None` when the sample is not shipped.
    fn sample(&self, reference: &str) -> Option<String>;
    fn filter_table(&self, reference: &str) -> bool;
    fn impulse(&self, reference: &str) -> bool;
    fn sequencer_source(&self, source: &str) -> bool;
    fn instance_kind(&self, kind: &str) -> bool;
}

/// What a promotion dropped. Empty means the object shipped whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromoteReport {
    pub skipped: Vec<String>,
}

impl PromoteReport {
    fn skip(&mut self, message: String) {
        self.skipped.push(message);
    }
}

/// Which factory directory a promotion writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryKind {
    Sound,
    Kit,
    RackPreset,
}

impl FactoryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sound => "Sound",
            Self::Kit => "kit",
            Self::RackPreset => "rack preset",
        }
    }

    fn dir(self, paths: &AppPaths) -> PathBuf {
        match self {
            Self::Sound => paths.factory_sounds_dir(),
            Self::Kit => paths.kits_dir(),
            Self::RackPreset => paths.rack_presets_dir(),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Sound => "sound",
            Self::Kit => "kit",
            Self::RackPreset => "rackpreset",
        }
    }
}

/// The file a promotion of `name` writes.
pub fn factory_path(paths: &AppPaths, kind: FactoryKind, name: &str) -> Result<PathBuf, String> {
    let stem = crate::project::sanitize_project_name(name);
    if stem.is_empty() {
        return Err(format!("A factory {} needs a name", kind.label()));
    }
    Ok(kind.dir(paths).join(format!("{stem}.{}", kind.extension())))
}

/// Promotion writes into the checkout's `content/`. An installed app's
/// factory root is its read-only bundle, so promotion is a dev-only tool.
pub fn ensure_writable_factory(paths: &AppPaths) -> Result<(), String> {
    if paths.is_release() {
        return Err(
            "Promote to factory only works from a development checkout (the app bundle is read-only)"
                .to_string(),
        );
    }
    Ok(())
}

/// Write a promoted object as pretty JSON so factory diffs review well.
pub fn write_factory_file<T: serde::Serialize>(
    paths: &AppPaths,
    kind: FactoryKind,
    name: &str,
    value: &T,
    overwrite: bool,
) -> Result<PathBuf, String> {
    ensure_writable_factory(paths)?;
    let path = factory_path(paths, kind, name)?;
    if path.exists() && !overwrite {
        return Err(format!("A factory {} named '{}' already exists", kind.label(), name.trim()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Could not serialize the {}: {error}", kind.label()))?;
    std::fs::write(&path, json)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    Ok(path)
}

/// Whether a Sound kept anything playable after sanitizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundOutcome {
    Kept,
    /// Every slot was skipped: there is nothing left to ship.
    Empty,
}

/// Drop every non-factory dependency from a Sound (or rack preset, or kit
/// pad — all three are a [`ProjectSoundPreset`]). `context` prefixes each
/// report line, e.g. `pad 'Kick'`.
pub fn sanitize_sound(
    sound: &mut ProjectSoundPreset,
    index: &dyn FactoryIndex,
    context: &str,
    report: &mut PromoteReport,
) -> SoundOutcome {
    let prefix = if context.is_empty() { String::new() } else { format!("{context}: ") };
    match &mut sound.track.kind {
        ProjectTrackKind::Rack { slots, .. } => {
            let slot_count = slots.len().max(sound.rack.slots.len());
            // Walk back to front so removals never shift a slot still to visit.
            for slot in (0..slot_count).rev() {
                let source = slots.get(slot);
                let pattern = sound.rack.slots.get(slot);
                let instrument_type = pattern
                    .map(|pattern| pattern.instrument_type)
                    .or_else(|| source.map(|source| source.instrument_type));
                let label = slot_label(slot, source, pattern);
                let drop_reason = match instrument_type {
                    Some(ProjectInstrumentType::Custom) => {
                        match source.and_then(|source| source.instrument_name.as_deref()) {
                            Some(name) if index.instrument(name) => None,
                            Some(name) => Some(format!("instrument '{name}' is not factory")),
                            None => Some("it names no instrument".to_string()),
                        }
                    }
                    Some(ProjectInstrumentType::Sampler) => {
                        let reference = pattern
                            .and_then(|pattern| pattern.sample_path.clone())
                            .or_else(|| source.and_then(|source| source.sample_path.clone()));
                        match reference {
                            // A blank sampler ships as a blank sampler.
                            None => None,
                            Some(reference) => match index.sample(&reference) {
                                Some(portable) => {
                                    if let Some(source) = slots.get_mut(slot) {
                                        source.sample_path = Some(portable.clone());
                                    }
                                    if let Some(pattern) = sound.rack.slots.get_mut(slot) {
                                        pattern.sample_path = Some(portable);
                                    }
                                    None
                                }
                                None => Some(format!(
                                    "sample '{}' is not a factory sample",
                                    sample_display(&reference)
                                )),
                            },
                        }
                    }
                    Some(ProjectInstrumentType::Rack) => {
                        Some("nested racks cannot be promoted".to_string())
                    }
                    _ => None,
                };
                if let Some(reason) = drop_reason {
                    report.skip(format!("{prefix}skipped {label} ({reason})"));
                    if slot < slots.len() {
                        slots.remove(slot);
                    }
                    remove_rack_slot(&mut sound.rack, slot);
                    continue;
                }
                if let Some(pattern) = sound.rack.slots.get_mut(slot) {
                    let slot_prefix = format!("{prefix}{label}: ");
                    let dropped = sanitize_effect_chain(
                        &mut pattern.custom_effects,
                        &mut pattern.effect_slots,
                        index,
                        &slot_prefix,
                        report,
                    );
                    for effect in dropped.into_iter().rev() {
                        remap_effect_removal(&mut sound.rack, slot, effect);
                    }
                }
            }
            if slots.is_empty() && sound.rack.slots.is_empty() {
                return SoundOutcome::Empty;
            }
            SoundOutcome::Kept
        }
        // Container captures always produce a rack track; anything else is a
        // shape this tool does not know how to vet.
        _ => {
            report.skip(format!("{prefix}skipped (not a rack container)"));
            SoundOutcome::Empty
        }
    }
}

/// Drop every non-factory dependency from a kit. Fails when no pad with a
/// sound survives, since an all-modulator kit is refused on save as well.
pub fn sanitize_kit(
    kit: &mut ProjectKitPreset,
    index: &dyn FactoryIndex,
    report: &mut PromoteReport,
) -> Result<(), String> {
    for pad_index in (0..kit.pads.len()).rev() {
        let pad = &mut kit.pads[pad_index];
        let context = format!("pad '{}'", pad_name(&pad.name, pad_index));
        let keep = match pad.sound.as_mut() {
            Some(sound) => {
                sanitize_sound(sound, index, &context, report) == SoundOutcome::Kept
            }
            None => true,
        };
        if keep {
            continue;
        }
        report.skip(format!("skipped {context} (nothing on it is factory)"));
        kit.pads.remove(pad_index);
        remove_kit_pad(kit, pad_index);
    }
    if kit.pads.iter().all(|pad| pad.sound.is_none()) {
        return Err("No pad of this kit is made from factory content".to_string());
    }

    if let Some(chain) = kit.bus_chain.as_mut() {
        chain.effects.retain(|effect| {
            match effect_drop_reason(&effect.name, &effect.slot, index) {
                Some(reason) => {
                    report.skip(format!("rack bus: skipped effect '{}' ({reason})", effect.name));
                    false
                }
                None => true,
            }
        });
    }

    kit.sequencers.retain(|sequencer| {
        if index.sequencer_source(&sequencer.source) {
            return true;
        }
        report.skip(format!(
            "skipped sequencer '{}' (its source is not in the factory tree)",
            sequencer.sequencer_name
        ));
        false
    });
    // Promotion captures kits without clips, so no clip override can point
    // at a sequencer dropped here.
    kit.instances.retain(|instance| {
        if index.instance_kind(&instance.kind) {
            return true;
        }
        let label = if instance.label.is_empty() { &instance.kind } else { &instance.label };
        report.skip(format!(
            "skipped sequencer '{label}' (package kind '{}' is not shipped)",
            instance.kind
        ));
        false
    });
    Ok(())
}

fn pad_name(name: &str, pad_index: usize) -> String {
    if name.trim().is_empty() { format!("#{}", pad_index + 1) } else { name.trim().to_string() }
}

fn slot_label(
    slot: usize,
    source: Option<&crate::project::ProjectRackTrackSlot>,
    pattern: Option<&crate::project::ProjectRackSlotPattern>,
) -> String {
    let what = source
        .and_then(|source| source.instrument_name.clone())
        .or_else(|| pattern.and_then(|pattern| pattern.sample_name.clone()))
        .or_else(|| source.and_then(|source| source.sample_name.clone()));
    match what {
        Some(what) if !what.trim().is_empty() => format!("slot {} ({})", slot + 1, what.trim()),
        _ => format!("slot {}", slot + 1),
    }
}

fn sample_display(reference: &str) -> String {
    Path::new(reference)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| reference.to_string())
}

/// Why an insert cannot ship, or `None` when it can.
fn effect_drop_reason(
    name: &str,
    slot: &ProjectEffectSlot,
    index: &dyn FactoryIndex,
) -> Option<String> {
    if !index.effect(name) {
        return Some("custom effect is not factory".to_string());
    }
    if let Some(table) = slot.table.as_deref().filter(|table| !table.is_empty()) {
        if !index.filter_table(table) {
            return Some(format!("filter table '{table}' is not factory"));
        }
    }
    if let Some(ir) = slot.ir.as_deref().filter(|ir| !ir.is_empty()) {
        if !index.impulse(ir) {
            return Some(format!("impulse response '{ir}' is not factory"));
        }
    }
    None
}

/// Remove non-factory inserts from one slot's chain and close the gaps, so
/// the chain stays contiguous. Returns the removed chain positions in
/// ascending order (pre-removal numbering).
fn sanitize_effect_chain(
    names: &mut Vec<Option<String>>,
    slots: &mut Vec<ProjectEffectSlot>,
    index: &dyn FactoryIndex,
    prefix: &str,
    report: &mut PromoteReport,
) -> Vec<usize> {
    let empty = ProjectEffectSlot::default();
    let mut removed = Vec::new();
    for position in 0..names.len() {
        let Some(name) = names[position].as_deref().map(str::trim).filter(|n| !n.is_empty())
        else {
            continue;
        };
        let slot = slots.get(position).unwrap_or(&empty);
        if let Some(reason) = effect_drop_reason(name, slot, index) {
            report.skip(format!("{prefix}skipped effect '{name}' ({reason})"));
            removed.push(position);
        }
    }
    let (names_len, slots_len) = (names.len(), slots.len());
    for &position in removed.iter().rev() {
        names.remove(position);
        if position < slots.len() {
            slots.remove(position);
        }
    }
    // Keep the chain's fixed width: loaders index these by slot position.
    names.resize(names_len, None);
    slots.resize_with(slots_len, ProjectEffectSlot::default);
    removed
}

/// Remove rack slot `slot` and renumber macro mappings that pointed past it.
fn remove_rack_slot(rack: &mut ProjectRackTrackPattern, slot: usize) {
    if slot < rack.slots.len() {
        rack.slots.remove(slot);
    }
    for mapping_macro in &mut rack.macros {
        mapping_macro.mappings.retain_mut(|mapping| {
            let target_slot = match &mut mapping.target {
                ProjectRackMacroTarget::SlotParam { slot, .. }
                | ProjectRackMacroTarget::SlotInstrumentParam { slot, .. }
                | ProjectRackMacroTarget::SlotEffectParam { slot, .. } => slot,
            };
            if *target_slot == slot {
                return false;
            }
            if *target_slot > slot {
                *target_slot -= 1;
            }
            true
        });
    }
}

/// Renumber macro mappings after insert `effect` of rack slot `slot` was
/// removed and the chain closed up.
fn remap_effect_removal(rack: &mut ProjectRackTrackPattern, slot: usize, effect: usize) {
    for mapping_macro in &mut rack.macros {
        mapping_macro.mappings.retain_mut(|mapping| {
            let ProjectRackMacroTarget::SlotEffectParam { slot: target, effect_slot, .. } =
                &mut mapping.target
            else {
                return true;
            };
            if *target != slot {
                return true;
            }
            if *effect_slot == effect {
                return false;
            }
            if *effect_slot > effect {
                *effect_slot -= 1;
            }
            true
        });
    }
}

/// Renumber pad-space references after pad `pad` was removed.
fn remove_kit_pad(kit: &mut ProjectKitPreset, pad: usize) {
    kit.mod_connections.retain_mut(|connection| {
        if connection.source_pad == pad {
            return false;
        }
        if connection.source_pad > pad {
            connection.source_pad -= 1;
        }
        match &mut connection.destination {
            ProjectKitModDestination::Pad(dest) if *dest == pad => false,
            ProjectKitModDestination::Pad(dest) => {
                if *dest > pad {
                    *dest -= 1;
                }
                true
            }
            ProjectKitModDestination::RackBus => true,
        }
    });
}

/// The [`FactoryIndex`] of the running checkout.
pub struct LiveFactoryIndex<'a> {
    paths: &'a AppPaths,
    factory_root: PathBuf,
    sample_hashes: HashSet<String>,
}

impl<'a> LiveFactoryIndex<'a> {
    pub fn new(paths: &'a AppPaths) -> Self {
        let factory_root = canonical(&paths.factory_root());
        Self { paths, factory_root, sample_hashes: factory_sample_hashes(paths) }
    }

    fn under_factory(&self, path: &Path) -> bool {
        path.exists() && canonical(path).starts_with(&self.factory_root)
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Content hashes of every sample the shipped packages carry.
fn factory_sample_hashes(paths: &AppPaths) -> HashSet<String> {
    let mut hashes = HashSet::new();
    let Ok(entries) = std::fs::read_dir(paths.factory_packages_dir()) else {
        return hashes;
    };
    for entry in entries.flatten() {
        let manifest = entry.path().join("samples.jsonl");
        let Ok(lines) = crate::sample_manifest::read_manifest(&manifest) else {
            continue;
        };
        for line in lines {
            if let crate::sample_manifest::SampleManifestLine::Sample(sample) = line {
                hashes.insert(sample.hash);
            }
        }
    }
    hashes
}

/// The `(load "…")` path of a recorded sequencer source, if it is one.
fn load_form_path(source: &str) -> Option<&str> {
    let inner = source.trim().strip_prefix("(load ")?.strip_suffix(')')?.trim();
    let path = inner.strip_prefix('"')?.strip_suffix('"')?;
    (!path.is_empty()).then_some(path)
}

impl FactoryIndex for LiveFactoryIndex<'_> {
    fn instrument(&self, id: &str) -> bool {
        crate::lisp_host::instrument_source_path(id)
            .is_ok_and(|source| self.under_factory(&source))
    }

    fn effect(&self, name: &str) -> bool {
        if crate::effects::builtin_effect_name_from_project_name(name).is_some() {
            return true;
        }
        self.under_factory(&crate::lisp_host::effect_source_path(name))
    }

    fn sample(&self, reference: &str) -> Option<String> {
        let stem = Path::new(reference).file_stem()?.to_str()?;
        self.sample_hashes.contains(stem).then(|| format!("samples/{stem}.wav"))
    }

    fn filter_table(&self, reference: &str) -> bool {
        let (reference, _engine) = crate::effects::filter_table::split_engine_ref(reference);
        let (name, _mode) = crate::effects::filter_table::decode_table_ref(reference);
        if name.is_empty() || name == crate::effects::filter_table::DEFAULT_TABLE_REF {
            return true;
        }
        match crate::effects::filter_table_asset::decode_asset_ref(name) {
            Some(stem) => {
                crate::effects::filter_table_asset::find_asset_in(&self.paths.filter_tables_dir(), stem)
                    .is_some()
            }
            // A table analyzed from a sample: shipped only if the sample is.
            None => self.sample_hashes.contains(name),
        }
    }

    fn impulse(&self, reference: &str) -> bool {
        reference == crate::effects::conv_reverb::DEFAULT_IR_REF
            || self.sample_hashes.contains(reference)
    }

    fn sequencer_source(&self, source: &str) -> bool {
        let Some(path) = load_form_path(source) else {
            // Inline script text travels inside the kit itself.
            return true;
        };
        let relative = path.strip_prefix("@/").unwrap_or(path);
        let path = Path::new(relative);
        if path.is_absolute() {
            return self.under_factory(path);
        }
        let stripped = path.strip_prefix("content").unwrap_or(path);
        self.paths.factory_root().join(stripped).is_file()
    }

    fn instance_kind(&self, kind: &str) -> bool {
        let Some((package, _kind)) = kind.split_once(':') else {
            return false;
        };
        let prefix = package.replace('/', ".");
        self.paths.factory_packages_dir().join(prefix).join("manifest.json").is_file()
    }
}
