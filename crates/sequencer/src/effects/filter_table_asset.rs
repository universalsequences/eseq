//! Versioned Filter Table asset format (`.fltab`).
//!
//! A Filter Table asset is a durable, original representation of an
//! authored or imported table: instead of re-decoding and re-analyzing an
//! audio file on every load, an asset stores the baked 64x1025 linear
//! magnitude bank the runtime actually consumes, plus versioned metadata.
//!
//! ## Container layout (format version 1)
//!
//! | bytes            | contents                                    |
//! |------------------|---------------------------------------------|
//! | 0..8             | magic `b"FLTABLE\n"`                        |
//! | 8..12            | u32 LE: JSON header length in bytes         |
//! | 12..12+len       | UTF-8 JSON header ([`FilterTableAssetMeta`]) |
//! | remainder        | frames*bins f32 LE linear magnitudes, row-major by frame |
//!
//! The header is human-readable JSON so assets can be inspected and
//! diffed; the payload is raw little-endian floats so round-trips are
//! bit-exact. `format_version` gates parsing: readers reject versions they
//! do not understand rather than guessing. Runtime always consumes baked
//! linear magnitudes; authoring tools (eseq-dtx.7/.8) may carry their
//! dB-domain recipe in `recipe` without affecting playback.
//!
//! ## Where assets live
//!
//! User assets resolve by file stem under `filter-tables/` in the working
//! directory (the `samples/` convention); bundled factory assets ship in
//! the crate under `assets/filter-tables/`. A project references an asset
//! as `fltab:<stem>` in the effect's persisted `table` field.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::filter_table::{MagnitudeTable, FRAMES, NBINS, REFERENCE_HARMONIC};

pub const MAGIC: &[u8; 8] = b"FLTABLE\n";
pub const FORMAT_VERSION: u32 = 1;
pub const EXTENSION: &str = "fltab";

/// Prefix that marks a persisted Filter Table reference as an asset stem
/// (`"fltab:<stem>"`) rather than a sample name.
pub const REF_PREFIX: &str = "fltab:";

fn default_reference_harmonic() -> usize {
    REFERENCE_HARMONIC
}

/// Human-readable JSON header of a Filter Table asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterTableAssetMeta {
    /// Container format version; readers reject versions above their own.
    pub format_version: u32,
    /// Human-readable asset name for browsing/inspection (the device panel
    /// displays the file stem so undo/reload stay consistent).
    pub name: String,
    /// Payload dimensions; must match the runtime bank (64x1025 today) and
    /// are stored explicitly so a future runtime can accept other sizes.
    pub frames: usize,
    pub bins: usize,
    /// Table harmonic that the `cutoff` parameter pins to its own
    /// frequency. Per-asset metadata rather than an undocumented global;
    /// the current DSP supports only the default (24).
    #[serde(default = "default_reference_harmonic")]
    pub reference_harmonic: usize,
    /// Linear-domain magnitude floor used when the asset was authored in
    /// dB (0.0 for imported/analyzed tables).
    #[serde(default)]
    pub magnitude_floor: f32,
    /// Provenance: the analysis mode tag the table was imported with, if
    /// it came from audio analysis (`None` for authored/generated tables).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis_mode: Option<String>,
    /// Provenance: name of the source the table was analyzed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    /// Optional default control values (`frame`, `cutoff`, `resonance`,
    /// `mix`) a preset suggests when loaded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub default_controls: BTreeMap<String, f32>,
    /// Optional authoring recipe (dB-domain curves, generator parameters)
    /// for future editor/generator documents. Opaque to the runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<serde_json::Value>,
}

impl FilterTableAssetMeta {
    /// Metadata for a freshly baked 64x1025 table at current defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: name.into(),
            frames: FRAMES,
            bins: NBINS,
            reference_harmonic: REFERENCE_HARMONIC,
            magnitude_floor: 0.0,
            analysis_mode: None,
            source_name: None,
            default_controls: BTreeMap::new(),
            recipe: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FilterTableAsset {
    pub meta: FilterTableAssetMeta,
    pub table: MagnitudeTable,
}

pub fn is_asset_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

pub fn encode_asset_ref(stem: &str) -> String {
    format!("{REF_PREFIX}{stem}")
}

/// The asset stem when `reference` is an asset reference, else `None`.
pub fn decode_asset_ref(reference: &str) -> Option<&str> {
    reference.strip_prefix(REF_PREFIX)
}

/// Serialize an asset. Fails when the metadata contradicts the table (so a
/// written file always reads back) rather than writing a broken asset.
pub fn write_asset(
    path: &Path,
    meta: &FilterTableAssetMeta,
    table: &MagnitudeTable,
) -> Result<(), String> {
    if meta.format_version != FORMAT_VERSION {
        return Err(format!(
            "asset metadata declares format version {}, but this build writes version {FORMAT_VERSION}",
            meta.format_version
        ));
    }
    if meta.frames != FRAMES || meta.bins != NBINS {
        return Err(format!(
            "asset metadata declares {}x{} magnitudes, but the table is {FRAMES}x{NBINS}",
            meta.frames, meta.bins
        ));
    }
    let header = serde_json::to_vec_pretty(meta)
        .map_err(|error| format!("failed to encode asset metadata: {error}"))?;
    let mut bytes =
        Vec::with_capacity(MAGIC.len() + 4 + header.len() + table.data.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&u32::try_from(header.len()).map_err(|_| "asset metadata too large".to_string())?.to_le_bytes());
    bytes.extend_from_slice(&header);
    for value in table.data.iter() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes)
        .map_err(|error| format!("failed to write asset '{}': {error}", path.display()))
}

/// Read and fully validate an asset file. Every failure names what is
/// wrong and, where possible, what to do about it.
pub fn read_asset(path: &Path) -> Result<FilterTableAsset, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read asset '{}': {error}", path.display()))?;
    let display = path.display();
    if bytes.len() < MAGIC.len() + 4 {
        return Err(format!(
            "'{display}' is too short to be a Filter Table asset"
        ));
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(format!(
            "'{display}' is not a Filter Table asset (bad magic); expected a .{EXTENSION} file"
        ));
    }
    let header_len =
        u32::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 4].try_into().unwrap()) as usize;
    let header_start = MAGIC.len() + 4;
    let payload_start = header_start
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| {
            format!("'{display}' is truncated: header claims {header_len} bytes of metadata")
        })?;
    let meta: FilterTableAssetMeta =
        serde_json::from_slice(&bytes[header_start..payload_start]).map_err(|error| {
            format!("'{display}' has malformed asset metadata: {error}")
        })?;
    if meta.format_version > FORMAT_VERSION || meta.format_version == 0 {
        return Err(format!(
            "'{display}' uses asset format version {}, but this build supports version {FORMAT_VERSION}; \
             re-export the asset or update the application",
            meta.format_version
        ));
    }
    if meta.frames != FRAMES || meta.bins != NBINS {
        return Err(format!(
            "'{display}' declares a {}x{} table, but this build supports {FRAMES}x{NBINS}",
            meta.frames, meta.bins
        ));
    }
    if meta.reference_harmonic != REFERENCE_HARMONIC {
        return Err(format!(
            "'{display}' declares reference harmonic {}, but this build's DSP supports only {REFERENCE_HARMONIC}",
            meta.reference_harmonic
        ));
    }
    let payload = &bytes[payload_start..];
    let expected = meta.frames * meta.bins * 4;
    if payload.len() != expected {
        return Err(format!(
            "'{display}' magnitude payload is {} bytes, expected {expected} ({}x{} f32 values)",
            payload.len(),
            meta.frames,
            meta.bins
        ));
    }
    let data = payload
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let table = MagnitudeTable::new(data)
        .map_err(|error| format!("'{display}' is invalid: {error}"))?;
    Ok(FilterTableAsset { meta, table })
}

/// Bundled factory asset directory (`crates/sequencer/assets/filter-tables`).
pub fn bundled_asset_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/filter-tables")
}

/// User asset directory, resolved like `samples/`: relative to the working
/// directory.
pub fn user_asset_dir() -> PathBuf {
    PathBuf::from("filter-tables")
}

/// Find `<stem>.fltab` under `dir`, recursively.
pub fn find_asset_in(dir: &Path, stem: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_asset_in(&path, stem) {
                return Some(found);
            }
            continue;
        }
        if is_asset_path(&path)
            && path
                .file_stem()
                .and_then(|candidate| candidate.to_str())
                .is_some_and(|candidate| candidate == stem)
        {
            return Some(path);
        }
    }
    None
}

/// Resolve an asset stem to a file: user `filter-tables/` first, then the
/// bundled factory directory.
pub fn resolve_asset_path(stem: &str) -> Option<PathBuf> {
    find_asset_in(&user_asset_dir(), stem).or_else(|| find_asset_in(&bundled_asset_dir(), stem))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::filter_table::{default_table, TABLE_LEN};

    fn scratch_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fltab-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join(name)
    }

    fn full_meta() -> FilterTableAssetMeta {
        let mut meta = FilterTableAssetMeta::new("Round Trip");
        meta.analysis_mode = Some("wavetable".to_string());
        meta.source_name = Some("synthetic ramp".to_string());
        meta.magnitude_floor = 1.0e-4;
        meta.default_controls =
            BTreeMap::from([("cutoff".to_string(), 55.0), ("mix".to_string(), 100.0)]);
        meta.recipe = Some(serde_json::json!({
            "kind": "formant-morph",
            "db_floor": -80.0,
        }));
        meta
    }

    #[test]
    fn asset_round_trip_is_lossless_in_magnitudes_and_metadata() {
        let path = scratch_path("round-trip.fltab");
        let table = default_table();
        let meta = full_meta();
        write_asset(&path, &meta, &table).expect("write");
        let read = read_asset(&path).expect("read");
        assert_eq!(read.meta, meta);
        assert_eq!(
            read.table.data.as_slice(),
            table.data.as_slice(),
            "magnitude payload must round-trip bit-exactly"
        );
    }

    #[test]
    fn read_rejects_bad_magic_with_actionable_error() {
        let path = scratch_path("not-an-asset.fltab");
        std::fs::write(&path, b"RIFF....WAVEfmt not really").expect("write");
        let error = read_asset(&path).expect_err("bad magic must fail");
        assert!(error.contains("not a Filter Table asset"), "{error}");
    }

    #[test]
    fn read_rejects_newer_format_version() {
        let path = scratch_path("future-version.fltab");
        let mut meta = FilterTableAssetMeta::new("Future");
        write_asset(&path, &meta, &default_table()).expect("write");
        meta.format_version = FORMAT_VERSION + 1;
        // Rewrite the header bytes with the bumped version.
        let header = serde_json::to_vec_pretty(&meta).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        for value in default_table().data.iter() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let error = read_asset(&path).expect_err("future version must fail");
        assert!(
            error.contains("format version") && error.contains("supports version 1"),
            "{error}"
        );
    }

    #[test]
    fn read_rejects_dimension_mismatch() {
        let path = scratch_path("wrong-dims.fltab");
        let mut meta = FilterTableAssetMeta::new("Wrong Dims");
        meta.frames = 32;
        let error = write_asset(&path, &meta, &default_table()).expect_err("write must fail");
        assert!(error.contains("32x1025"), "{error}");
        // A file that lies about its dimensions on disk is rejected on read.
        let header = serde_json::to_vec_pretty(&meta).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        std::fs::write(&path, bytes).unwrap();
        let error = read_asset(&path).expect_err("dimension mismatch must fail");
        assert!(error.contains("supports 64x1025"), "{error}");
    }

    #[test]
    fn read_rejects_truncated_payload() {
        let path = scratch_path("truncated.fltab");
        write_asset(&path, &FilterTableAssetMeta::new("Trunc"), &default_table())
            .expect("write");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 12);
        std::fs::write(&path, &bytes).unwrap();
        let error = read_asset(&path).expect_err("truncated payload must fail");
        assert!(error.contains("payload"), "{error}");
    }

    #[test]
    fn read_rejects_nonfinite_magnitudes() {
        let path = scratch_path("nonfinite.fltab");
        write_asset(&path, &FilterTableAssetMeta::new("NaN"), &default_table())
            .expect("write");
        let mut bytes = std::fs::read(&path).unwrap();
        let payload_start = bytes.len() - TABLE_LEN * 4;
        bytes[payload_start..payload_start + 4]
            .copy_from_slice(&f32::NAN.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let error = read_asset(&path).expect_err("NaN payload must fail");
        assert!(error.contains("invalid"), "{error}");
    }

    #[test]
    fn read_rejects_unsupported_reference_harmonic() {
        let path = scratch_path("harmonic.fltab");
        let mut meta = FilterTableAssetMeta::new("Harmonic 32");
        meta.reference_harmonic = 32;
        write_asset(&path, &meta, &default_table()).expect("write");
        let error = read_asset(&path).expect_err("unsupported harmonic must fail");
        assert!(
            error.contains("reference harmonic 32") && error.contains("only 24"),
            "{error}"
        );
    }

    #[test]
    fn asset_refs_encode_and_decode() {
        assert_eq!(encode_asset_ref("vowel-morph"), "fltab:vowel-morph");
        assert_eq!(decode_asset_ref("fltab:vowel-morph"), Some("vowel-morph"));
        assert_eq!(decode_asset_ref("vowel-morph"), None);
    }

    #[test]
    fn find_asset_in_resolves_by_stem_recursively() {
        let dir = scratch_path("resolve-dir");
        let nested = dir.join("factory/morphs");
        std::fs::create_dir_all(&nested).expect("nested dir");
        let path = nested.join("comb-city.fltab");
        write_asset(&path, &FilterTableAssetMeta::new("Comb City"), &default_table())
            .expect("write");
        assert_eq!(find_asset_in(&dir, "comb-city"), Some(path));
        assert_eq!(find_asset_in(&dir, "missing"), None);
    }
}
