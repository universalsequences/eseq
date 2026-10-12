//! Shared lane values for project files (file version 17).
//!
//! Scene patterns, rack clips, orphan sounds and take chunks repeat the same
//! per-track values: a drum rack's sound is identical in every clip and every
//! scene that plays it. Live state shares them through `Arc`s; a file
//! written naively spells each copy out (36 MB for a project whose distinct
//! content is a few). Inside [`write_project`], the per-track columns of
//! [`ProjectPattern`](super::ProjectPattern) annotated with [`serialize`]
//! write each large value once into a top-level `lane_table` and put its
//! index in the column instead; [`read_project`] resolves the indices.
//!
//! Anywhere else (kits, sound presets, in-memory round trips) the columns
//! serialize inline, exactly as before, and inline columns always load.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;

use serde::de::{DeserializeOwned, Error as _};
use serde::ser::{Error as _, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ProjectFile;

/// Smaller values stay inline: an index would save too little to pay for
/// the indirection.
const MIN_SHARED_BYTES: usize = 128;

thread_local! {
    static ACTIVE: RefCell<Option<Table>> = const { RefCell::new(None) };
}

enum Table {
    Writing {
        index: HashMap<String, usize>,
        entries: Vec<serde_json::Value>,
    },
    Reading {
        entries: Vec<serde_json::Value>,
        /// Decoded values by entry and type: most entries are read many
        /// times, and cloning beats decoding again.
        decoded: HashMap<(usize, TypeId), Box<dyn Any>>,
    },
}

/// Installs a table for the current thread and removes it on drop, so an
/// error or panic mid-(de)serialization never leaks it into a later one.
struct Activation;

impl Activation {
    fn new(table: Table) -> Self {
        ACTIVE.with(|active| *active.borrow_mut() = Some(table));
        Self
    }

    fn take(self) -> Option<Table> {
        ACTIVE.with(|active| active.borrow_mut().take())
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.borrow_mut().take());
    }
}

/// Write `project` with its large lane values in a shared `lane_table`.
pub(super) fn write_project<W: std::io::Write>(
    writer: W,
    project: &ProjectFile,
) -> serde_json::Result<()> {
    #[derive(Serialize)]
    struct Saved<'a> {
        #[serde(flatten)]
        project: &'a ProjectFile,
        // Serialized after every pattern, so it holds all their entries.
        lane_table: CollectedEntries,
    }
    let activation = Activation::new(Table::Writing {
        index: HashMap::new(),
        entries: Vec::new(),
    });
    let result = serde_json::to_writer(
        writer,
        &Saved {
            project,
            lane_table: CollectedEntries,
        },
    );
    drop(activation);
    result
}

/// Read a project written by [`write_project`] (or any older file).
pub(super) fn read_project(json: &str) -> serde_json::Result<ProjectFile> {
    #[derive(Deserialize)]
    struct TableOnly {
        #[serde(default)]
        lane_table: Vec<serde_json::Value>,
    }
    let entries = serde_json::from_str::<TableOnly>(json)?.lane_table;
    let activation = Activation::new(Table::Reading {
        entries,
        decoded: HashMap::new(),
    });
    let project = serde_json::from_str(json);
    drop(activation);
    project
}

/// The writing table's entries, emitted as the file's `lane_table`.
struct CollectedEntries;

impl Serialize for CollectedEntries {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let entries = ACTIVE.with(|active| match active.borrow_mut().as_mut() {
            Some(Table::Writing { entries, .. }) => std::mem::take(entries),
            _ => Vec::new(),
        });
        entries.serialize(serializer)
    }
}

fn is_writing() -> bool {
    ACTIVE.with(|active| matches!(*active.borrow(), Some(Table::Writing { .. })))
}

/// `serialize_with` for a per-track column: each large lane becomes a table
/// index while [`write_project`] runs.
pub(super) fn serialize<T, S>(lanes: &[T], serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    if !is_writing() {
        return lanes.serialize(serializer);
    }
    let mut seq = serializer.serialize_seq(Some(lanes.len()))?;
    for lane in lanes {
        let json = serde_json::to_string(lane).map_err(S::Error::custom)?;
        // A bare number is how an index reads back, so a lane that encodes
        // as one is always shared (none of the annotated types does).
        let numeric = json.starts_with(|c: char| c == '-' || c.is_ascii_digit());
        if json.len() < MIN_SHARED_BYTES && !numeric {
            seq.serialize_element(lane)?;
            continue;
        }
        let shared = ACTIVE.with(|active| match active.borrow_mut().as_mut() {
            Some(Table::Writing { index, entries }) => {
                if let Some(&at) = index.get(&json) {
                    return Ok::<usize, serde_json::Error>(at);
                }
                let value = serde_json::from_str(&json)?;
                entries.push(value);
                index.insert(json, entries.len() - 1);
                Ok(entries.len() - 1)
            }
            _ => unreachable!("checked by is_writing"),
        });
        seq.serialize_element(&shared.map_err(S::Error::custom)?)?;
    }
    seq.end()
}

/// `deserialize_with` for a per-track column: inline lanes decode as
/// themselves, table indices resolve through the reading table.
pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Clone + 'static,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|lane| match lane {
            serde_json::Value::Number(number) => {
                let at = number.as_u64().ok_or_else(|| {
                    D::Error::custom(format!("invalid lane table index {number}"))
                })?;
                shared_lane(at as usize).map_err(D::Error::custom)
            }
            inline => T::deserialize(inline).map_err(D::Error::custom),
        })
        .collect()
}

enum Lookup<T> {
    Cached(T),
    Entry(serde_json::Value),
}

fn shared_lane<T: DeserializeOwned + Clone + 'static>(at: usize) -> Result<T, String> {
    let key = (at, TypeId::of::<T>());
    // Look up under the borrow, but decode outside it: a lane type that
    // itself held an indexed column would otherwise re-enter the table.
    let entry = ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(Table::Reading { entries, decoded }) = active.as_ref() else {
            return Err(format!("lane table index {at} outside a project file"));
        };
        if let Some(value) = decoded
            .get(&key)
            .and_then(|value| value.downcast_ref::<T>())
        {
            return Ok(Lookup::Cached(value.clone()));
        }
        entries
            .get(at)
            .cloned()
            .map(Lookup::Entry)
            .ok_or_else(|| format!("lane table index {at} out of range ({})", entries.len()))
    })?;
    let entry = match entry {
        Lookup::Entry(entry) => entry,
        Lookup::Cached(value) => return Ok(value),
    };
    let value = T::deserialize(entry).map_err(|error| error.to_string())?;
    ACTIVE.with(|active| {
        if let Some(Table::Reading { decoded, .. }) = active.borrow_mut().as_mut() {
            decoded.insert(key, Box::new(value.clone()));
        }
    });
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value repeated across columns is stored once; small and inline
    /// values round-trip untouched, and the same JSON read without a table
    /// (kits, presets) is plain inline JSON.
    #[test]
    fn repeated_lanes_share_one_entry_and_round_trip() {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        struct Columns {
            #[serde(serialize_with = "serialize", deserialize_with = "deserialize")]
            big: Vec<Option<Vec<u32>>>,
            #[serde(serialize_with = "serialize", deserialize_with = "deserialize")]
            other: Vec<Option<Vec<u32>>>,
        }
        let large: Vec<u32> = (0..64).collect();
        let columns = Columns {
            big: vec![
                Some(large.clone()),
                None,
                Some(vec![1]),
                Some(large.clone()),
            ],
            other: vec![Some(large.clone())],
        };

        let activation = Activation::new(Table::Writing {
            index: HashMap::new(),
            entries: Vec::new(),
        });
        let json = serde_json::to_value(&columns).unwrap();
        let Some(Table::Writing { entries, .. }) = activation.take() else {
            panic!("writing table");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(json["big"], serde_json::json!([0, null, [1], 0]));
        assert_eq!(json["other"], serde_json::json!([0]));

        let activation = Activation::new(Table::Reading {
            entries,
            decoded: HashMap::new(),
        });
        let restored: Columns = serde_json::from_value(json.clone()).unwrap();
        drop(activation);
        assert_eq!(restored, columns);

        let error = serde_json::from_value::<Columns>(json).unwrap_err();
        assert!(
            error.to_string().contains("outside a project file"),
            "{error}"
        );
        let inline = serde_json::to_value(&columns).unwrap();
        assert_eq!(serde_json::from_value::<Columns>(inline).unwrap(), columns);
    }
}
