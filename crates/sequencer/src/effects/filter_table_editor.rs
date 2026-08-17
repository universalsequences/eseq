//! Nondestructive Filter Table response editor (eseq-dtx.8).
//!
//! The editor is a document/command model independent of any widget:
//! [`EditorDoc`] holds frame-indexed frequency-response magnitudes in dB
//! (floor [`DB_FLOOR`]) plus an ordered [`EditOp`] history with an undo
//! cursor. Every mutation is an op; undo/redo replays the history against
//! the immutable base. [`EditorDoc::bake`] resamples the document's frames
//! to the runtime's 64 rows and converts to validated linear magnitudes —
//! the exact table the DSP consumes, so "displayed response" and "runtime
//! response" are the same numbers by construction.
//!
//! A document serializes into the `.fltab` asset `recipe` field
//! ([`EditorDoc::snapshot`]): the asset payload carries the baked result
//! (playable everywhere), while the recipe carries base + ops so a saved
//! table reopens with its full nondestructive history.
//!
//! [`EditorSession`] (one at a time, like the analysis-mode UI it sits
//! beside) binds a document to a live device node for auditioning: session
//! previews write the baked table straight to the node's tensor and
//! published visualization bank without touching the undo/persistence
//! registries; save goes through the recorded-mutation path like any other
//! table load.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::filter_table::{FRAMES, MagnitudeTable, NBINS, REFERENCE_HARMONIC, TABLE_LEN};

/// Editing floor: dB values at or below this bake to exactly 0.0 linear.
pub const DB_FLOOR: f32 = -80.0;
/// Ceiling keeps draws/parametric stacks from running away.
pub const DB_CEIL: f32 = 24.0;
pub const DOC_KIND: &str = "filter-table-editor-doc";
pub const DOC_VERSION: u32 = 1;
/// Frame insertion cap; the runtime always sees 64 resampled rows.
pub const MAX_DOC_FRAMES: usize = 256;

fn db_from_linear(value: f32) -> f32 {
    if value <= 0.0 {
        DB_FLOOR
    } else {
        (20.0 * value.log10()).clamp(DB_FLOOR, DB_CEIL)
    }
}

fn linear_from_db(db: f32) -> f32 {
    if db <= DB_FLOOR + 1.0e-3 {
        0.0
    } else {
        10.0_f32.powf(db.clamp(DB_FLOOR, DB_CEIL) / 20.0)
    }
}

/// Octave coordinate of a bin relative to the cutoff-pinned reference
/// harmonic (bin 24 = 0 octaves), matching the preset generator's axis so
/// parametric edits transpose with `cutoff` the same way presets do.
fn bin_octave(bin: usize) -> f32 {
    ((bin.max(1)) as f32 / REFERENCE_HARMONIC as f32).log2()
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawPoint {
    pub bin: usize,
    pub db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParametricKind {
    Peak,
    Notch,
    Lowpass,
    Highpass,
    Tilt,
}

impl ParametricKind {
    pub const ALL: [ParametricKind; 5] = [
        ParametricKind::Peak,
        ParametricKind::Notch,
        ParametricKind::Lowpass,
        ParametricKind::Highpass,
        ParametricKind::Tilt,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            ParametricKind::Peak => "peak",
            ParametricKind::Notch => "notch",
            ParametricKind::Lowpass => "lowpass",
            ParametricKind::Highpass => "highpass",
            ParametricKind::Tilt => "tilt",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.tag() == tag)
    }

    /// Sensible starting node for "add a <kind> node" in the UI.
    pub fn default_node(self) -> ParametricNode {
        let gain_db = match self {
            ParametricKind::Peak => 6.0,
            ParametricKind::Notch => -18.0,
            ParametricKind::Lowpass | ParametricKind::Highpass => -12.0,
            ParametricKind::Tilt => 3.0,
        };
        ParametricNode {
            kind: self,
            center_oct: 1.0,
            width_oct: 1.0,
            gain_db,
        }
    }
}

/// A draggable response node: additive dB shape on the octave axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParametricNode {
    pub kind: ParametricKind,
    /// Center position in octaves relative to the reference harmonic.
    pub center_oct: f32,
    /// Bandwidth in octaves (gaussian sigma is half of this); for the
    /// pass/tilt kinds it softens nothing — slope is `gain_db` per octave.
    pub width_oct: f32,
    pub gain_db: f32,
}

impl ParametricNode {
    fn delta_db(&self, oct: f32) -> f32 {
        match self.kind {
            ParametricKind::Peak | ParametricKind::Notch => {
                let sigma = (self.width_oct * 0.5).max(1.0e-3);
                let z = (oct - self.center_oct) / sigma;
                self.gain_db * (-0.5 * z * z).exp()
            }
            ParametricKind::Lowpass => {
                -self.gain_db.abs() * (oct - self.center_oct).max(0.0)
            }
            ParametricKind::Highpass => {
                -self.gain_db.abs() * (self.center_oct - oct).max(0.0)
            }
            ParametricKind::Tilt => self.gain_db * (oct - self.center_oct),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let finite = self.center_oct.is_finite()
            && self.width_oct.is_finite()
            && self.gain_db.is_finite();
        if !finite {
            return Err("parametric node has non-finite parameters".to_string());
        }
        if !(0.01..=8.0).contains(&self.width_oct) {
            return Err(format!(
                "parametric node width {} octaves is outside 0.01..=8",
                self.width_oct
            ));
        }
        Ok(())
    }
}

/// One undoable edit. Frame ranges are inclusive and validated against the
/// document's frame count at the time the op applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum EditOp {
    /// Pencil: set dB values on one frame, linearly interpolated between
    /// the given (sorted-by-bin) points.
    Draw { frame: usize, points: Vec<DrawPoint> },
    /// Additive parametric node over a frame range.
    Parametric {
        frame_start: usize,
        frame_end: usize,
        node: ParametricNode,
    },
    /// Insert a new frame at `at`, interpolated from its neighbors (edge
    /// inserts duplicate the edge frame).
    InsertFrame { at: usize },
    DuplicateFrame { at: usize },
    DeleteFrame { at: usize },
    MoveFrame { from: usize, to: usize },
    /// Keyframe interpolation: frames strictly between `start` and `end`
    /// become linear dB blends of the two anchor frames.
    InterpolateFrames { start: usize, end: usize },
    /// Box smoothing along the frequency axis (radius in bins, dB domain).
    SmoothSpectral {
        frame_start: usize,
        frame_end: usize,
        radius: usize,
    },
    /// Box smoothing across frames (radius in frames, dB domain).
    SmoothTemporal {
        frame_start: usize,
        frame_end: usize,
        radius: usize,
    },
    /// Translate the response along the log-frequency axis.
    ShiftOctaves {
        frame_start: usize,
        frame_end: usize,
        octaves: f32,
    },
    /// Stretch the response about the reference harmonic in log-frequency.
    StretchOctaves {
        frame_start: usize,
        frame_end: usize,
        factor: f32,
    },
    Tilt {
        frame_start: usize,
        frame_end: usize,
        db_per_octave: f32,
    },
    /// Per-frame peak to 0 dB.
    Normalize { frame_start: usize, frame_end: usize },
}

impl EditOp {
    fn frame_range(&self) -> Option<(usize, usize)> {
        match *self {
            EditOp::Parametric {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::SmoothSpectral {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::SmoothTemporal {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::ShiftOctaves {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::StretchOctaves {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::Tilt {
                frame_start,
                frame_end,
                ..
            }
            | EditOp::Normalize {
                frame_start,
                frame_end,
            } => Some((frame_start, frame_end)),
            _ => None,
        }
    }

    fn validate(&self, frames: usize) -> Result<(), String> {
        if let Some((start, end)) = self.frame_range() {
            if start > end || end >= frames {
                return Err(format!(
                    "frame range {start}..={end} is invalid for a {frames}-frame document"
                ));
            }
        }
        match self {
            EditOp::Draw { frame, points } => {
                if *frame >= frames {
                    return Err(format!("draw frame {frame} is out of range"));
                }
                if points.is_empty() {
                    return Err("draw needs at least one point".to_string());
                }
                for pair in points.windows(2) {
                    if pair[1].bin <= pair[0].bin {
                        return Err("draw points must have strictly increasing bins".to_string());
                    }
                }
                if points
                    .iter()
                    .any(|point| point.bin >= NBINS || !point.db.is_finite())
                {
                    return Err("draw point is out of range or non-finite".to_string());
                }
                Ok(())
            }
            EditOp::Parametric { node, .. } => node.validate(),
            EditOp::InsertFrame { at } | EditOp::DuplicateFrame { at } => {
                if *at >= frames {
                    return Err(format!("frame {at} is out of range"));
                }
                if frames >= MAX_DOC_FRAMES {
                    return Err(format!("document already has {MAX_DOC_FRAMES} frames"));
                }
                Ok(())
            }
            EditOp::DeleteFrame { at } => {
                if *at >= frames {
                    return Err(format!("frame {at} is out of range"));
                }
                if frames <= 1 {
                    return Err("cannot delete the last frame".to_string());
                }
                Ok(())
            }
            EditOp::MoveFrame { from, to } => {
                if *from >= frames || *to >= frames {
                    return Err("move frame is out of range".to_string());
                }
                Ok(())
            }
            EditOp::InterpolateFrames { start, end } => {
                if *start >= frames || *end >= frames || start + 1 >= *end {
                    return Err(
                        "keyframe interpolation needs two anchors with frames between them"
                            .to_string(),
                    );
                }
                Ok(())
            }
            EditOp::SmoothSpectral { radius, .. } => {
                if *radius == 0 || *radius > NBINS / 2 {
                    return Err(format!("spectral radius {radius} is out of range"));
                }
                Ok(())
            }
            EditOp::SmoothTemporal { radius, .. } => {
                if *radius == 0 || *radius > frames {
                    return Err(format!("temporal radius {radius} is out of range"));
                }
                Ok(())
            }
            EditOp::ShiftOctaves { octaves, .. } => {
                if !octaves.is_finite() || octaves.abs() > 6.0 {
                    return Err("shift must be within +/-6 octaves".to_string());
                }
                Ok(())
            }
            EditOp::StretchOctaves { factor, .. } => {
                if !factor.is_finite() || !(0.125..=8.0).contains(factor) {
                    return Err("stretch factor must be within 0.125..=8".to_string());
                }
                Ok(())
            }
            EditOp::Tilt { db_per_octave, .. } => {
                if !db_per_octave.is_finite() || db_per_octave.abs() > 48.0 {
                    return Err("tilt must be within +/-48 dB per octave".to_string());
                }
                Ok(())
            }
            EditOp::Normalize { .. } => Ok(()),
        }
    }

    fn apply(&self, rows: &mut Vec<Vec<f32>>) {
        match self {
            EditOp::Draw { frame, points } => {
                let row = &mut rows[*frame];
                if points.len() == 1 {
                    row[points[0].bin] = points[0].db.clamp(DB_FLOOR, DB_CEIL);
                }
                for pair in points.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    let span = (b.bin - a.bin) as f32;
                    for bin in a.bin..=b.bin {
                        let t = (bin - a.bin) as f32 / span;
                        row[bin] = (a.db + (b.db - a.db) * t).clamp(DB_FLOOR, DB_CEIL);
                    }
                }
            }
            EditOp::Parametric {
                frame_start,
                frame_end,
                node,
            } => {
                for row in &mut rows[*frame_start..=*frame_end] {
                    for (bin, value) in row.iter_mut().enumerate() {
                        *value = (*value + node.delta_db(bin_octave(bin)))
                            .clamp(DB_FLOOR, DB_CEIL);
                    }
                }
            }
            EditOp::InsertFrame { at } => {
                let new_row = if *at == 0 {
                    rows[0].clone()
                } else {
                    rows[*at - 1]
                        .iter()
                        .zip(rows[*at].iter())
                        .map(|(a, b)| 0.5 * (a + b))
                        .collect()
                };
                rows.insert(*at, new_row);
            }
            EditOp::DuplicateFrame { at } => {
                let row = rows[*at].clone();
                rows.insert(*at + 1, row);
            }
            EditOp::DeleteFrame { at } => {
                rows.remove(*at);
            }
            EditOp::MoveFrame { from, to } => {
                let row = rows.remove(*from);
                rows.insert(*to, row);
            }
            EditOp::InterpolateFrames { start, end } => {
                let span = (*end - *start) as f32;
                let (head, tail) = rows.split_at_mut(*end);
                let end_row = &tail[0];
                let start_row = head[*start].clone();
                for frame in *start + 1..*end {
                    let t = (frame - *start) as f32 / span;
                    for bin in 0..NBINS {
                        head[frame][bin] =
                            start_row[bin] + (end_row[bin] - start_row[bin]) * t;
                    }
                }
            }
            EditOp::SmoothSpectral {
                frame_start,
                frame_end,
                radius,
            } => {
                for row in &mut rows[*frame_start..=*frame_end] {
                    let source = row.clone();
                    for bin in 0..NBINS {
                        let lo = bin.saturating_sub(*radius);
                        let hi = (bin + radius).min(NBINS - 1);
                        let sum: f32 = source[lo..=hi].iter().sum();
                        row[bin] = sum / (hi - lo + 1) as f32;
                    }
                }
            }
            EditOp::SmoothTemporal {
                frame_start,
                frame_end,
                radius,
            } => {
                let source = rows.clone();
                for frame in *frame_start..=*frame_end {
                    let lo = frame.saturating_sub(*radius).max(*frame_start);
                    let hi = (frame + radius).min(*frame_end);
                    let count = (hi - lo + 1) as f32;
                    for bin in 0..NBINS {
                        let sum: f32 =
                            source[lo..=hi].iter().map(|row| row[bin]).sum();
                        rows[frame][bin] = sum / count;
                    }
                }
            }
            EditOp::ShiftOctaves {
                frame_start,
                frame_end,
                octaves,
            } => {
                let scale = 2.0_f32.powf(-octaves);
                for row in &mut rows[*frame_start..=*frame_end] {
                    let source = row.clone();
                    for (bin, value) in row.iter_mut().enumerate().skip(1) {
                        *value = sample_row(&source, bin as f32 * scale);
                    }
                }
            }
            EditOp::StretchOctaves {
                frame_start,
                frame_end,
                factor,
            } => {
                let reference = REFERENCE_HARMONIC as f32;
                for row in &mut rows[*frame_start..=*frame_end] {
                    let source = row.clone();
                    for (bin, value) in row.iter_mut().enumerate().skip(1) {
                        let position =
                            reference * (bin as f32 / reference).powf(1.0 / factor);
                        *value = sample_row(&source, position);
                    }
                }
            }
            EditOp::Tilt {
                frame_start,
                frame_end,
                db_per_octave,
            } => {
                for row in &mut rows[*frame_start..=*frame_end] {
                    for (bin, value) in row.iter_mut().enumerate() {
                        *value = (*value + db_per_octave * bin_octave(bin))
                            .clamp(DB_FLOOR, DB_CEIL);
                    }
                }
            }
            EditOp::Normalize {
                frame_start,
                frame_end,
            } => {
                for row in &mut rows[*frame_start..=*frame_end] {
                    let peak = row.iter().cloned().fold(DB_FLOOR, f32::max);
                    if peak > DB_FLOOR {
                        for value in row.iter_mut() {
                            *value = (*value - peak).max(DB_FLOOR);
                        }
                    }
                }
            }
        }
    }
}

/// Linear interpolation into a dB row at a fractional bin position, edge
/// clamped (matches the DSP's clamped gathers rather than wrapping).
fn sample_row(row: &[f32], position: f32) -> f32 {
    let position = position.clamp(0.0, (NBINS - 1) as f32);
    let base = position.floor() as usize;
    let next = (base + 1).min(NBINS - 1);
    let frac = position - base as f32;
    row[base] + (row[next] - row[base]) * frac
}

/// Nondestructive editor document: immutable base + op history + cursor.
#[derive(Clone, Debug)]
pub struct EditorDoc {
    base: Vec<Vec<f32>>,
    ops: Vec<EditOp>,
    cursor: usize,
    current: Vec<Vec<f32>>,
}

impl EditorDoc {
    /// Start a document from the table currently loaded in the device.
    pub fn from_table(table: &MagnitudeTable) -> Self {
        let base: Vec<Vec<f32>> = (0..FRAMES)
            .map(|frame| {
                table.data[frame * NBINS..(frame + 1) * NBINS]
                    .iter()
                    .map(|value| db_from_linear(*value))
                    .collect()
            })
            .collect();
        let current = base.clone();
        Self {
            base,
            ops: Vec::new(),
            cursor: 0,
            current,
        }
    }

    pub fn frame_count(&self) -> usize {
        self.current.len()
    }

    pub fn op_count(&self) -> usize {
        self.cursor
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.ops.len()
    }

    pub fn ops(&self) -> &[EditOp] {
        &self.ops
    }

    /// The applied op history's last entry, if any (for UI band display).
    pub fn last_applied(&self) -> Option<&EditOp> {
        self.cursor.checked_sub(1).and_then(|index| self.ops.get(index))
    }

    /// dB value grid of the current state (frames x NBINS).
    pub fn current_rows(&self) -> &[Vec<f32>] {
        &self.current
    }

    /// Apply a new op: validates, truncates any redo tail, pushes.
    pub fn apply(&mut self, op: EditOp) -> Result<(), String> {
        op.validate(self.current.len())?;
        self.ops.truncate(self.cursor);
        op.apply(&mut self.current);
        self.ops.push(op);
        self.cursor += 1;
        Ok(())
    }

    /// Replace the newest applied op (drag coalescing: one gesture, one
    /// undo entry). Fails over to `apply` when there is nothing to replace.
    pub fn replace_last(&mut self, op: EditOp) -> Result<(), String> {
        if self.cursor == 0 || self.cursor != self.ops.len() {
            return self.apply(op);
        }
        let saved = self.ops.pop().expect("cursor > 0 implies an op");
        self.cursor -= 1;
        self.recompute();
        match self.apply(op) {
            Ok(()) => Ok(()),
            Err(error) => {
                // Put the original back so a bad replacement is a no-op.
                self.ops.push(saved);
                self.cursor += 1;
                self.recompute();
                Err(error)
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.recompute();
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.cursor >= self.ops.len() {
            return false;
        }
        self.ops[self.cursor].apply(&mut self.current);
        self.cursor += 1;
        true
    }

    fn recompute(&mut self) {
        self.current = self.base.clone();
        for op in &self.ops[..self.cursor] {
            op.apply(&mut self.current);
        }
    }

    /// Bake the current state to the validated runtime representation:
    /// resample the document's K frames to the runtime's 64 (frame f sits
    /// at fractional position `f*(K-1)/63`, linear dB interpolation — the
    /// same deterministic mapping structured wavetable import uses), then
    /// convert dB to linear with the floor mapping to exactly 0.0.
    pub fn bake(&self) -> Result<MagnitudeTable, String> {
        let frames = self.current.len();
        let mut data = Vec::with_capacity(TABLE_LEN);
        for out_frame in 0..FRAMES {
            let position = if FRAMES == 1 {
                0.0
            } else {
                out_frame as f32 * (frames - 1) as f32 / (FRAMES - 1) as f32
            };
            let base = (position.floor() as usize).min(frames - 1);
            let next = (base + 1).min(frames - 1);
            let frac = position - base as f32;
            for bin in 0..NBINS {
                let db =
                    self.current[base][bin] + (self.current[next][bin] - self.current[base][bin]) * frac;
                data.push(linear_from_db(db));
            }
        }
        MagnitudeTable::new(data)
    }

    /// Bake with one uncommitted op overlaid (live drag preview).
    pub fn bake_with_preview(&self, op: &EditOp) -> Result<MagnitudeTable, String> {
        op.validate(self.current.len())?;
        let mut preview = self.clone();
        op.apply(&mut preview.current);
        preview.bake()
    }

    /// Serialize the document (base + applied ops; any redo tail is
    /// dropped, as on every save boundary) into an asset `recipe` value.
    pub fn snapshot(&self) -> serde_json::Value {
        let flat: Vec<f32> = self.base.iter().flatten().copied().collect();
        serde_json::json!({
            "kind": DOC_KIND,
            "version": DOC_VERSION,
            "db_floor": DB_FLOOR,
            "base_frames": self.base.len(),
            "base_db_b64": base64_encode(&f32s_to_le_bytes(&flat)),
            "ops": serde_json::to_value(&self.ops[..self.cursor]).expect("ops serialize"),
        })
    }

    /// Rebuild a document from an asset recipe written by [`snapshot`].
    pub fn from_snapshot(recipe: &serde_json::Value) -> Result<Self, String> {
        if recipe.get("kind").and_then(|kind| kind.as_str()) != Some(DOC_KIND) {
            return Err("recipe is not an editor document".to_string());
        }
        let version = recipe
            .get("version")
            .and_then(|version| version.as_u64())
            .unwrap_or(0);
        if version == 0 || version > DOC_VERSION as u64 {
            return Err(format!(
                "editor document version {version} is unsupported (this build reads version {DOC_VERSION})"
            ));
        }
        let base_frames = recipe
            .get("base_frames")
            .and_then(|frames| frames.as_u64())
            .ok_or_else(|| "editor document is missing base_frames".to_string())?
            as usize;
        if base_frames == 0 || base_frames > MAX_DOC_FRAMES {
            return Err(format!("editor document base has {base_frames} frames"));
        }
        let bytes = base64_decode(
            recipe
                .get("base_db_b64")
                .and_then(|base| base.as_str())
                .ok_or_else(|| "editor document is missing base_db_b64".to_string())?,
        )
        .ok_or_else(|| "editor document base is not valid base64".to_string())?;
        let flat = le_bytes_to_f32s(&bytes);
        if flat.len() != base_frames * NBINS {
            return Err(format!(
                "editor document base has {} values, expected {}",
                flat.len(),
                base_frames * NBINS
            ));
        }
        if flat.iter().any(|value| !value.is_finite()) {
            return Err("editor document base contains non-finite dB values".to_string());
        }
        let base: Vec<Vec<f32>> = flat.chunks(NBINS).map(|chunk| chunk.to_vec()).collect();
        let ops: Vec<EditOp> = recipe
            .get("ops")
            .map(|ops| {
                serde_json::from_value(ops.clone())
                    .map_err(|error| format!("editor document ops are malformed: {error}"))
            })
            .transpose()?
            .unwrap_or_default();
        let cursor = ops.len();
        let mut doc = Self {
            current: base.clone(),
            base,
            ops,
            cursor,
        };
        // Replay validates every op against the evolving frame count so a
        // tampered recipe cannot index out of range.
        doc.cursor = 0;
        doc.current = doc.base.clone();
        let replay = std::mem::take(&mut doc.ops);
        for op in replay {
            doc.apply(op)
                .map_err(|error| format!("editor document op replay failed: {error}"))?;
        }
        Ok(doc)
    }
}

fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn le_bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Minimal standard base64 (no padding shortcuts skipped, no deps).
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|byte| **byte == b'=').count();
        let mut triple: u32 = 0;
        for (index, byte) in chunk.iter().enumerate() {
            let v = if *byte == b'=' {
                if index < 2 {
                    return None;
                }
                0
            } else {
                value(*byte)?
            };
            triple |= v << (18 - 6 * index);
        }
        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad < 1 {
            out.push(triple as u8);
        }
    }
    Some(out)
}

/// Which live device an editor session is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorTarget {
    Track { track: usize, slot: usize },
    Bus { bus: usize, slot: usize },
}

/// One active editing session (a single session at a time, like the
/// analysis-mode affordance beside it).
pub struct EditorSession {
    pub target: EditorTarget,
    pub node_id: i32,
    pub doc: EditorDoc,
    pub selected_frame: usize,
    /// Table/reference/name the device had when the editor opened, so
    /// closing without saving restores exactly what was there.
    pub original_table: std::sync::Arc<MagnitudeTable>,
    pub original_ref: String,
    pub original_name: String,
    pub dirty: bool,
}

static SESSION: Mutex<Option<EditorSession>> = Mutex::new(None);

pub fn with_session<T>(
    reader: impl FnOnce(Option<&mut EditorSession>) -> T,
) -> T {
    let mut guard = SESSION.lock().expect("editor session lock");
    reader(guard.as_mut())
}

pub fn set_session(session: Option<EditorSession>) -> Option<EditorSession> {
    let mut guard = SESSION.lock().expect("editor session lock");
    std::mem::replace(&mut *guard, session)
}

/// Session info for the effects-panel value builder: `(target, node_id,
/// frame_count, selected_frame, can_undo, can_redo, dirty, last parametric
/// node if that is the newest op)`.
pub struct SessionUiState {
    pub target: EditorTarget,
    pub node_id: i32,
    pub frames: usize,
    pub selected_frame: usize,
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    pub op_count: usize,
    pub band: Option<ParametricNode>,
}

pub fn session_ui_state() -> Option<SessionUiState> {
    with_session(|session| {
        session.map(|session| SessionUiState {
            target: session.target,
            node_id: session.node_id,
            frames: session.doc.frame_count(),
            selected_frame: session.selected_frame,
            can_undo: session.doc.can_undo(),
            can_redo: session.doc.can_redo(),
            dirty: session.dirty,
            op_count: session.doc.op_count(),
            band: match session.doc.last_applied() {
                Some(EditOp::Parametric { node, .. }) => Some(*node),
                _ => None,
            },
        })
    })
}

/// Default asset metadata for a document saved as `name`.
pub fn save_meta(name: &str, doc: &EditorDoc) -> super::filter_table_asset::FilterTableAssetMeta {
    let mut meta = super::filter_table_asset::FilterTableAssetMeta::new(name);
    meta.magnitude_floor = 0.0;
    meta.source_name = Some("filter-table editor".to_string());
    meta.default_controls = BTreeMap::new();
    meta.recipe = Some(doc.snapshot());
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::filter_table::default_table;

    fn flat_doc(db: f32) -> EditorDoc {
        let linear = linear_from_db(db);
        let table = MagnitudeTable::new(vec![linear; TABLE_LEN]).expect("flat table");
        EditorDoc::from_table(&table)
    }

    #[test]
    fn draw_sets_interpolated_db_values_on_one_frame() {
        let mut doc = flat_doc(-12.0);
        doc.apply(EditOp::Draw {
            frame: 3,
            points: vec![
                DrawPoint { bin: 10, db: 0.0 },
                DrawPoint { bin: 14, db: -8.0 },
            ],
        })
        .expect("draw");
        let rows = doc.current_rows();
        assert_eq!(rows[3][10], 0.0);
        assert_eq!(rows[3][14], -8.0);
        assert!((rows[3][12] - -4.0).abs() < 1.0e-4, "midpoint interpolates");
        assert_eq!(rows[2][12], -12.0, "other frames untouched");
        assert_eq!(rows[3][9], -12.0, "outside the span untouched");
    }

    #[test]
    fn parametric_peak_is_additive_and_undoable() {
        let mut doc = flat_doc(-20.0);
        let node = ParametricNode {
            kind: ParametricKind::Peak,
            center_oct: 0.0,
            width_oct: 1.0,
            gain_db: 6.0,
        };
        doc.apply(EditOp::Parametric {
            frame_start: 0,
            frame_end: 63,
            node,
        })
        .expect("parametric");
        let at_center = doc.current_rows()[0][REFERENCE_HARMONIC];
        assert!((at_center - -14.0).abs() < 0.05, "center gains +6 dB: {at_center}");
        assert!(doc.undo());
        assert_eq!(doc.current_rows()[0][REFERENCE_HARMONIC], -20.0);
        assert!(doc.redo());
        assert!((doc.current_rows()[0][REFERENCE_HARMONIC] - -14.0).abs() < 0.05);
    }

    #[test]
    fn frame_ops_insert_duplicate_delete_move_and_interpolate() {
        let mut doc = flat_doc(-10.0);
        doc.apply(EditOp::Draw {
            frame: 0,
            points: vec![DrawPoint { bin: 5, db: 0.0 }],
        })
        .expect("mark frame 0");
        assert_eq!(doc.frame_count(), 64);
        doc.apply(EditOp::DuplicateFrame { at: 0 }).expect("dup");
        assert_eq!(doc.frame_count(), 65);
        assert_eq!(doc.current_rows()[1][5], 0.0, "duplicate copies the row");
        doc.apply(EditOp::InsertFrame { at: 1 }).expect("insert");
        assert_eq!(doc.current_rows()[1][5], 0.0, "insert interpolates equals");
        doc.apply(EditOp::MoveFrame { from: 0, to: 2 }).expect("move");
        assert_eq!(doc.current_rows()[2][5], 0.0);
        doc.apply(EditOp::DeleteFrame { at: 2 }).expect("delete");
        assert_eq!(doc.frame_count(), 65);

        // Keyframe interpolation: set anchors and check the midpoint.
        let mut doc = flat_doc(-10.0);
        doc.apply(EditOp::Draw {
            frame: 10,
            points: vec![DrawPoint { bin: 7, db: 0.0 }],
        })
        .expect("anchor a");
        doc.apply(EditOp::Draw {
            frame: 20,
            points: vec![DrawPoint { bin: 7, db: -20.0 }],
        })
        .expect("anchor b");
        doc.apply(EditOp::InterpolateFrames { start: 10, end: 20 })
            .expect("interpolate");
        let mid = doc.current_rows()[15][7];
        assert!((mid - -10.0).abs() < 1.0e-3, "midpoint blends anchors: {mid}");
    }

    #[test]
    fn shift_stretch_tilt_normalize_behave_on_the_octave_axis() {
        // A single bright bin at the reference harmonic.
        let mut doc = flat_doc(-40.0);
        doc.apply(EditOp::Draw {
            frame: 0,
            points: vec![DrawPoint {
                bin: REFERENCE_HARMONIC,
                db: 0.0,
            }],
        })
        .expect("seed peak");
        doc.apply(EditOp::ShiftOctaves {
            frame_start: 0,
            frame_end: 0,
            octaves: 1.0,
        })
        .expect("shift");
        let row = &doc.current_rows()[0];
        assert!(
            row[REFERENCE_HARMONIC * 2] > -1.0,
            "peak moved up one octave: {}",
            row[REFERENCE_HARMONIC * 2]
        );
        assert!(row[REFERENCE_HARMONIC] < -30.0, "old position emptied");

        // Stretch by 2 about the reference: the shifted peak (1 oct above
        // reference) lands 2 octaves above.
        doc.apply(EditOp::StretchOctaves {
            frame_start: 0,
            frame_end: 0,
            factor: 2.0,
        })
        .expect("stretch");
        assert!(
            doc.current_rows()[0][REFERENCE_HARMONIC * 4] > -1.0,
            "stretch doubles the octave distance"
        );

        let mut doc = flat_doc(-10.0);
        doc.apply(EditOp::Tilt {
            frame_start: 0,
            frame_end: 63,
            db_per_octave: 6.0,
        })
        .expect("tilt");
        let row = &doc.current_rows()[5];
        assert!((row[REFERENCE_HARMONIC] - -10.0).abs() < 0.05, "pivot unmoved");
        assert!(
            (row[REFERENCE_HARMONIC * 2] - -4.0).abs() < 0.05,
            "one octave up gains 6 dB"
        );

        doc.apply(EditOp::Normalize {
            frame_start: 0,
            frame_end: 63,
        })
        .expect("normalize");
        for row in doc.current_rows() {
            let peak = row.iter().cloned().fold(f32::MIN, f32::max);
            assert!((peak - 0.0).abs() < 1.0e-3, "peak sits at 0 dB: {peak}");
        }
    }

    #[test]
    fn smoothing_reduces_variation_spectrally_and_temporally() {
        let mut doc = flat_doc(-30.0);
        doc.apply(EditOp::Draw {
            frame: 8,
            points: vec![DrawPoint { bin: 100, db: 0.0 }],
        })
        .expect("spike");
        doc.apply(EditOp::SmoothSpectral {
            frame_start: 8,
            frame_end: 8,
            radius: 2,
        })
        .expect("smooth spectral");
        let row = &doc.current_rows()[8];
        assert!(row[100] < -5.0, "spike spread out: {}", row[100]);
        assert!(row[99] > -30.0 && row[101] > -30.0, "neighbors received energy");

        doc.apply(EditOp::SmoothTemporal {
            frame_start: 0,
            frame_end: 63,
            radius: 1,
        })
        .expect("smooth temporal");
        assert!(
            doc.current_rows()[7][100] > -30.0 && doc.current_rows()[9][100] > -30.0,
            "adjacent frames received energy"
        );
    }

    #[test]
    fn replace_last_coalesces_a_drag_into_one_undo_entry() {
        let mut doc = flat_doc(-20.0);
        let mut node = ParametricKind::Peak.default_node();
        doc.apply(EditOp::Parametric {
            frame_start: 0,
            frame_end: 63,
            node,
        })
        .expect("first");
        for _ in 0..5 {
            node.gain_db += 1.0;
            doc.replace_last(EditOp::Parametric {
                frame_start: 0,
                frame_end: 63,
                node,
            })
            .expect("coalesce");
        }
        assert_eq!(doc.op_count(), 1, "one gesture, one undo entry");
        assert!(doc.undo());
        assert_eq!(doc.current_rows()[0][REFERENCE_HARMONIC], -20.0);
    }

    #[test]
    fn bake_matches_dc_free_round_trip_and_respects_the_floor() {
        let table = default_table();
        let doc = EditorDoc::from_table(&table);
        let baked = doc.bake().expect("bake");
        for (index, (a, b)) in table.data.iter().zip(baked.data.iter()).enumerate() {
            // dB round-trip is not bit-exact, but must stay well inside any
            // audible tolerance; exact zeros must stay exactly zero.
            if *a <= 10.0_f32.powf(DB_FLOOR / 20.0) {
                // At or below the editing floor everything bakes to silence.
                assert_eq!(*b, 0.0, "sub-floor magnitude bakes to zero at {index}");
            } else {
                let ratio = b / a;
                assert!(
                    (0.999..=1.001).contains(&ratio),
                    "bin {index}: {a} -> {b} drifts more than 0.1%"
                );
            }
        }

        let doc = flat_doc(DB_FLOOR);
        let baked = doc.bake().expect("bake floor");
        assert!(baked.data.iter().all(|value| *value == 0.0), "floor bakes to silence");
    }

    #[test]
    fn variable_frame_documents_bake_through_the_deterministic_mapping() {
        // Two-frame document: closed and open. Baked frame f blends them at
        // f/63.
        let mut doc = flat_doc(-60.0);
        // Reduce to a 2-frame document.
        while doc.frame_count() > 2 {
            doc.apply(EditOp::DeleteFrame { at: doc.frame_count() - 1 })
                .expect("shrink");
        }
        doc.apply(EditOp::Draw {
            frame: 1,
            points: vec![
                DrawPoint { bin: 1, db: 0.0 },
                DrawPoint { bin: 1024, db: 0.0 },
            ],
        })
        .expect("open frame");
        let baked = doc.bake().expect("bake");
        let db_at = |frame: usize, bin: usize| db_from_linear(baked.data[frame * NBINS + bin]);
        assert!((db_at(0, 100) - -60.0).abs() < 0.1, "first frame closed");
        assert!((db_at(63, 100) - 0.0).abs() < 0.1, "last frame open");
        let expected_mid = -60.0 + (0.0 - -60.0) * (31.0 / 63.0);
        assert!(
            (db_at(31, 100) - expected_mid).abs() < 0.5,
            "frame 31 blends deterministically: {} vs {expected_mid}",
            db_at(31, 100)
        );
    }

    #[test]
    fn snapshot_round_trips_document_state_and_bake_bit_exactly() {
        let mut doc = EditorDoc::from_table(&default_table());
        doc.apply(EditOp::Parametric {
            frame_start: 0,
            frame_end: 63,
            node: ParametricKind::Notch.default_node(),
        })
        .expect("op 1");
        doc.apply(EditOp::Tilt {
            frame_start: 10,
            frame_end: 20,
            db_per_octave: -3.0,
        })
        .expect("op 2");
        doc.apply(EditOp::DuplicateFrame { at: 5 }).expect("op 3");
        let snapshot = doc.snapshot();
        let restored = EditorDoc::from_snapshot(&snapshot).expect("restore");
        assert_eq!(restored.op_count(), 3);
        assert_eq!(restored.frame_count(), doc.frame_count());
        assert_eq!(
            doc.bake().expect("bake a").data.as_slice(),
            restored.bake().expect("bake b").data.as_slice(),
            "restored document bakes bit-exactly"
        );
        // Undo still works after restore (base survived the round trip).
        let mut restored = restored;
        assert!(restored.undo() && restored.undo() && restored.undo());
        assert_eq!(
            restored.bake().expect("bake base").data.as_slice(),
            EditorDoc::from_table(&default_table())
                .bake()
                .expect("bake fresh")
                .data
                .as_slice(),
        );
    }

    #[test]
    fn snapshot_rejects_malformed_and_tampered_documents() {
        let doc = EditorDoc::from_table(&default_table());
        let mut snapshot = doc.snapshot();
        snapshot["version"] = serde_json::json!(99);
        assert!(EditorDoc::from_snapshot(&snapshot)
            .expect_err("future version")
            .contains("version"));

        let mut snapshot = doc.snapshot();
        snapshot["base_db_b64"] = serde_json::json!("not base64!!!");
        assert!(EditorDoc::from_snapshot(&snapshot).is_err());

        // An op indexing past the frame count must fail replay, not panic.
        let mut snapshot = doc.snapshot();
        snapshot["ops"] = serde_json::json!([
            {"op": "delete-frame", "at": 900}
        ]);
        assert!(EditorDoc::from_snapshot(&snapshot)
            .expect_err("tampered op")
            .contains("replay"));
    }

    #[test]
    fn invalid_ops_are_rejected_with_actionable_errors() {
        let mut doc = flat_doc(-10.0);
        assert!(doc
            .apply(EditOp::Draw { frame: 99, points: vec![DrawPoint { bin: 0, db: 0.0 }] })
            .is_err());
        assert!(doc.apply(EditOp::Draw { frame: 0, points: vec![] }).is_err());
        assert!(doc
            .apply(EditOp::ShiftOctaves { frame_start: 0, frame_end: 63, octaves: f32::NAN })
            .is_err());
        assert!(doc
            .apply(EditOp::StretchOctaves { frame_start: 0, frame_end: 63, factor: 0.0 })
            .is_err());
        assert!(doc
            .apply(EditOp::Normalize { frame_start: 40, frame_end: 20 })
            .is_err());
        while doc.frame_count() > 1 {
            doc.apply(EditOp::DeleteFrame { at: 0 }).expect("shrink");
        }
        assert!(doc.apply(EditOp::DeleteFrame { at: 0 }).is_err(), "last frame stays");
        assert_eq!(doc.op_count(), 63, "rejected ops never enter the history");
    }

    #[test]
    fn base64_round_trips_arbitrary_bytes() {
        for len in [0usize, 1, 2, 3, 4, 65, 256] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            let encoded = base64_encode(&bytes);
            assert_eq!(base64_decode(&encoded).expect("decode"), bytes, "len {len}");
        }
        assert!(base64_decode("###").is_none());
    }
}
