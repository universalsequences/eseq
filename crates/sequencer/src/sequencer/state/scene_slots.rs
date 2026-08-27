use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::process::ProcessLiteral;

// Epochs cross the UI/scheduler VM boundary in snapshots, so they must not be
// counters local to one pattern: two scenes can each have a first write with
// different values. A process-wide monotonic source makes the token sufficient
// to distinguish those resolutions while scene duplication can safely copy an
// unchanged token together with its unchanged value.
static NEXT_SCENE_SLOT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Values larger than this are accepted, but authoring natives surface a
/// warning because every overriding pattern serializes its own copy.
pub const SCENE_SLOT_SOFT_SERIALIZED_BYTES: usize = 64 * 1024;

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

    /// Return an authoring diagnostic without rejecting the value. The
    /// reported byte count uses the same serde representation embedded in
    /// project JSON, rather than an in-memory estimate that could miss
    /// serialization bloat.
    ///
    /// Every authored write pays this check — including writes from
    /// scheduler-side script callbacks — so the measuring serialization is
    /// gated behind an allocation-free lower bound on that same encoding: a
    /// value that cannot be over the cap never allocates. Measuring never
    /// fails a write; portability is `validate_scene_slot_literal`'s job, and
    /// it reports an authoring error naming the declaration.
    pub fn soft_size_diagnostic(name: &str, value: &ProcessLiteral) -> Option<String> {
        let lower_bound = min_serialized_bytes(value);
        if lower_bound <= SCENE_SLOT_SOFT_SERIALIZED_BYTES {
            return None;
        }
        let bytes = serde_json::to_vec(value)
            .map(|json| json.len())
            .unwrap_or(lower_bound);
        Some(format!(
            "Scene slot '{}' stores {} serialized bytes in every overriding pattern (soft cap: {} bytes); the value was stored",
            name, bytes, SCENE_SLOT_SOFT_SERIALIZED_BYTES
        ))
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
            // The generation is an invalidation token, not an "is overridden"
            // flag: it must keep reporting the last write even after an undo
            // removed the override, or a memoizing reader would see every
            // non-overridden slot in every scene share epoch 0.
            None => ResolvedSceneSlot {
                value: declaration_default,
                epoch: self.epoch(name),
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

/// A lower bound on `serde_json`'s byte length for `value`.
///
/// Each arm counts the externally-tagged wrapper serde always emits plus the
/// shortest possible payload, and omits separators and string escaping, so the
/// result can only undershoot. Exceeding the soft cap here therefore always
/// means the exact measurement exceeds it too.
fn min_serialized_bytes(value: &ProcessLiteral) -> usize {
    match value {
        // `"Nil"`
        ProcessLiteral::Nil => 5,
        // `{"Number":0}` / `{"Bool":true}`
        ProcessLiteral::Number(_) => 12,
        ProcessLiteral::Bool(_) => 13,
        // `{"String":"…"}` and its equally long `Symbol`/`Keyword` siblings
        ProcessLiteral::String(text)
        | ProcessLiteral::Symbol(text)
        | ProcessLiteral::Keyword(text) => 13 + text.len(),
        // `{"List":[…]}`
        ProcessLiteral::List(items) => {
            11 + items.iter().map(min_serialized_bytes).sum::<usize>()
        }
        // `{"Map":{"key":…}}`
        ProcessLiteral::Map(items) => {
            10 + items
                .iter()
                .map(|(key, value)| 3 + key.len() + min_serialized_bytes(value))
                .sum::<usize>()
        }
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

    #[test]
    fn serialized_size_cap_is_soft_and_reports_the_slot() {
        let value = ProcessLiteral::String("x".repeat(SCENE_SLOT_SOFT_SERIALIZED_BYTES));
        let diagnostic = SceneSlotStore::soft_size_diagnostic("figures", &value)
            .expect("serde tagging pushes this value over the soft cap");
        assert!(diagnostic.contains("Scene slot 'figures'"), "{diagnostic}");
        assert!(diagnostic.contains("the value was stored"), "{diagnostic}");

        let mut store = SceneSlotStore::default();
        store
            .write_literal("figures", value.clone())
            .expect("the cap must not reject a portable value");
        assert_eq!(store.get("figures"), Some(&value));
    }

    /// The cheap gate must never claim more than serde emits, or a value under
    /// the cap would be reported as over it.
    #[test]
    fn the_size_gate_never_overshoots_the_serde_encoding() {
        let values = [
            ProcessLiteral::Nil,
            ProcessLiteral::Bool(false),
            ProcessLiteral::Number(0.0),
            ProcessLiteral::Number(-1234.5678),
            ProcessLiteral::String(String::new()),
            ProcessLiteral::Symbol("figure".to_string()),
            ProcessLiteral::Keyword("mode".to_string()),
            ProcessLiteral::List(Vec::new()),
            ProcessLiteral::Map(BTreeMap::new()),
            ProcessLiteral::List(vec![
                ProcessLiteral::Number(1.0),
                ProcessLiteral::Nil,
                ProcessLiteral::Map(BTreeMap::from([(
                    "a".to_string(),
                    ProcessLiteral::List(vec![ProcessLiteral::Bool(true)]),
                )])),
            ]),
        ];
        for value in values {
            let exact = serde_json::to_vec(&value).expect("portable values encode").len();
            assert!(
                min_serialized_bytes(&value) <= exact,
                "{value:?}: bound {} exceeds serde's {exact}",
                min_serialized_bytes(&value)
            );
        }
    }

    /// A list of small numbers is far larger encoded than its payload, so the
    /// gate has to count the per-element tagging it cannot skip.
    #[test]
    fn the_size_gate_counts_per_element_tagging() {
        let value = ProcessLiteral::List(
            (0..SCENE_SLOT_SOFT_SERIALIZED_BYTES / 8)
                .map(|index| ProcessLiteral::Number(index as f64))
                .collect(),
        );
        assert!(
            SceneSlotStore::soft_size_diagnostic("figures", &value).is_some(),
            "tagging alone pushes a compact list of numbers over the cap"
        );
        assert!(
            SceneSlotStore::soft_size_diagnostic(
                "figures",
                &ProcessLiteral::List(vec![ProcessLiteral::Number(1.0)])
            )
            .is_none(),
            "ordinary values stay quiet"
        );
    }
}
