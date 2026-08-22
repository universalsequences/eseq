use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::ProcessLiteral;

// Epochs cross the UI/scheduler VM boundary in snapshots, so they must not be
// counters local to one pattern: two scenes can each have a first write with
// different values. A process-wide monotonic source makes the token sufficient
// to distinguish those resolutions while scene duplication can safely copy an
// unchanged token together with its unchanged value.
static NEXT_SCENE_SLOT_EPOCH: AtomicU64 = AtomicU64::new(1);

fn next_scene_slot_epoch() -> u64 {
    NEXT_SCENE_SLOT_EPOCH
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
            epoch.checked_add(1)
        })
        .expect("scene-slot epoch space exhausted")
}

/// Pattern-scoped values written through `defscene` declarations.
///
/// Values and write generations travel together in immutable pattern and
/// scheduler snapshots. Only values are serialized; generations are runtime
/// cache invalidation tokens and are re-seeded when a project is loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneSlotStore {
    values: BTreeMap<String, ProcessLiteral>,
    epochs: BTreeMap<String, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedSceneSlot<'a> {
    pub value: &'a ProcessLiteral,
    pub epoch: u64,
    pub overridden: bool,
}

impl SceneSlotStore {
    /// Reject a declaration default before it can be resolved as a slot
    /// value; stored writes are validated by `write_literal`.
    pub(crate) fn validate_literal(name: &str, value: &ProcessLiteral) -> Result<(), String> {
        validate_scene_slot_literal(name, value)
    }

    pub fn from_values(values: BTreeMap<String, ProcessLiteral>) -> Result<Self, String> {
        for (name, value) in &values {
            validate_scene_slot_literal(name, value)?;
        }
        let epochs = values
            .keys()
            .cloned()
            .map(|name| (name, next_scene_slot_epoch()))
            .collect();
        Ok(Self { values, epochs })
    }

    pub fn values(&self) -> &BTreeMap<String, ProcessLiteral> {
        &self.values
    }

    pub fn into_values(self) -> BTreeMap<String, ProcessLiteral> {
        self.values
    }

    pub fn get(&self, name: &str) -> Option<&ProcessLiteral> {
        self.values.get(name)
    }

    pub fn epoch(&self, name: &str) -> u64 {
        self.epochs.get(name).copied().unwrap_or(0)
    }

    /// Resolve an override, falling back directly to the declaration default.
    pub fn resolve<'a>(
        &'a self,
        name: &str,
        declaration_default: &'a ProcessLiteral,
    ) -> ResolvedSceneSlot<'a> {
        match self.values.get(name) {
            Some(value) => ResolvedSceneSlot {
                value,
                epoch: self.epoch(name),
                overridden: true,
            },
            None => ResolvedSceneSlot {
                value: declaration_default,
                epoch: 0,
                overridden: false,
            },
        }
    }

    /// Replace an override and advance this slot's generation. `None` removes
    /// the override so history can faithfully restore declaration fallback;
    /// it is not equivalent to storing the declaration's current default.
    pub fn set_override(
        &mut self,
        name: impl Into<String>,
        value: Option<ProcessLiteral>,
    ) -> Result<u64, String> {
        let name = name.into();
        if let Some(value) = &value {
            validate_scene_slot_literal(&name, value)?;
        }
        let epoch = next_scene_slot_epoch();
        match value {
            Some(value) => {
                self.values.insert(name.clone(), value);
            }
            None => {
                self.values.remove(&name);
            }
        }
        self.epochs.insert(name, epoch);
        Ok(epoch)
    }

    /// Store a validated literal and advance this slot's generation even when
    /// the new value compares equal to the old value. Consumers may therefore
    /// safely invalidate on every authored write rather than memoizing the
    /// first resolution.
    pub fn write_literal(
        &mut self,
        name: impl Into<String>,
        value: ProcessLiteral,
    ) -> Result<u64, String> {
        self.set_override(name, Some(value))
    }

    /// Convert and store a VM value, reporting an authoring error that names
    /// the declaration rather than leaking the process-literal implementation
    /// vocabulary into the surface language.
    pub fn write_value(
        &mut self,
        name: impl Into<String>,
        value: &eseqlisp::vm::Value,
    ) -> Result<u64, String> {
        let name = name.into();
        let literal = ProcessLiteral::from_value(value)
            .map_err(|error| format!("scene slot '{}': {}", name, error))?;
        self.write_literal(name, literal)
    }
}

fn validate_scene_slot_literal(name: &str, value: &ProcessLiteral) -> Result<(), String> {
    fn validate(value: &ProcessLiteral) -> bool {
        match value {
            ProcessLiteral::Number(value) => value.is_finite(),
            ProcessLiteral::List(items) => items.iter().all(validate),
            ProcessLiteral::Map(items) => items.values().all(validate),
            ProcessLiteral::Bool(_)
            | ProcessLiteral::Nil
            | ProcessLiteral::String(_)
            | ProcessLiteral::Symbol(_)
            | ProcessLiteral::Keyword(_) => true,
        }
    }

    if validate(value) {
        Ok(())
    } else {
        Err(format!(
            "scene slot '{}': values must contain only portable finite literals",
            name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::vm::Value;

    #[test]
    fn resolution_prefers_override_and_every_write_bumps_its_epoch() {
        let default = ProcessLiteral::Number(2.0);
        let mut store = SceneSlotStore::default();
        let unresolved = store.resolve("rate", &default);
        assert_eq!(unresolved.value, &default);
        assert_eq!(unresolved.epoch, 0);
        assert!(!unresolved.overridden);

        let first_epoch = store
            .write_literal("rate", ProcessLiteral::Number(4.0))
            .unwrap();
        let second_epoch = store
            .write_literal("rate", ProcessLiteral::Number(4.0))
            .unwrap();
        assert!(second_epoch > first_epoch);
        let resolved = store.resolve("rate", &default);
        assert_eq!(resolved.value, &ProcessLiteral::Number(4.0));
        assert_eq!(resolved.epoch, second_epoch);
        assert!(resolved.overridden);
    }

    #[test]
    fn epochs_distinguish_writes_to_the_same_slot_in_different_patterns() {
        let mut first = SceneSlotStore::default();
        let mut second = SceneSlotStore::default();
        let first_epoch = first
            .write_literal("rate", ProcessLiteral::Number(2.0))
            .unwrap();
        let second_epoch = second
            .write_literal("rate", ProcessLiteral::Number(3.0))
            .unwrap();
        assert_ne!(first_epoch, second_epoch);
    }

    #[test]
    fn invalid_vm_value_error_names_the_slot() {
        let mut store = SceneSlotStore::default();
        let error = store
            .write_value("figures", &Value::Function(0))
            .expect_err("native functions are not portable literals");
        assert!(error.contains("scene slot 'figures'"), "{error}");
    }

    #[test]
    fn non_finite_nested_number_is_rejected_before_serialization() {
        let mut store = SceneSlotStore::default();
        let error = store
            .write_literal(
                "figures",
                ProcessLiteral::List(vec![ProcessLiteral::Number(f64::NAN)]),
            )
            .expect_err("NaN cannot round-trip through project JSON");
        assert!(error.contains("scene slot 'figures'"), "{error}");
    }
}
