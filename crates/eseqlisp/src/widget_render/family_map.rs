//! `family-map`: a sound family plotted on its axes, plus a preview of what
//! the current morph sounds like.
//!
//! Built for the VILLAIN drums: each instrument is a family of fitted hits
//! (33 kicks, 32 snares, 12 hats) that Kick A / Kick B pick from and Blend /
//! Exaggerate morph between. The widget reads a `family.json` sidecar next to
//! the instrument and draws:
//!
//! - every member as a dot at (axis 1, axis 2), sized by axis 3;
//! - members A and B (the `a` / `b` params) highlighted and tagged;
//! - the A-B line with the blend point riding it, a dashed lead from there to
//!   where Exaggerate and the three axis offsets push the sound, and a ring
//!   there whose size follows axis 3;
//! - under the plot, a strip previewing the morph.
//!
//! The preview is one of:
//! - modal (kick, snare): dual-maintained with the generated dsp.lisp. Per
//!   slot it redoes blend -> exaggerate -> per-axis offsets -> clip with the
//!   means / clip ranges / offsets written into the DSP, then the voice's knob
//!   transforms, and synthesizes the first 400 ms of the tonal bank (the noise
//!   layers and colour chain are not simulated), normalized to its own peak;
//! - lattice (hat, whose model has no closed form): peak envelopes of real
//!   renders of the instrument at fixed blend steps for every pair,
//!   interpolated between the two nearest steps.
//!
//! Clicking a dot dispatches `(on-pick index slot)`: `slot` is the `armed`
//! prop (0 = A, 1 = B), flipped by Shift. Drawing is plain primitives plus one
//! anti-aliased mesh, so there is no WGSL/MSL pair to keep in sync.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::stroke::ShadedMesh;
use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive,
    MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetViewport, get_f32_prop,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct FamilyMapWidget;

pub static FAMILY_MAP_WIDGET: FamilyMapWidget = FamilyMapWidget;

/// Each axis is fitted to the family's furthest member times this margin, but
/// never tighter than `MIN_EXTENT_SD` (so a narrow family is not blown up).
const EXTENT_MARGIN: f32 = 1.12;
const MIN_EXTENT_SD: f32 = 2.0;
/// Pick radius around a dot, in design pixels.
const PICK_RADIUS_PX: f32 = 12.0;
/// Preview window and synthesis rate. Slots above `PREVIEW_FMAX` would alias at
/// this rate and are left out; the preview is a picture of the sub and body.
const PREVIEW_SECONDS: f32 = 0.4;
const PREVIEW_RATE: f32 = 4000.0;
const PREVIEW_FMAX: f32 = 1800.0;
/// The preview strip under the plot: a share of the widget's height, at least
/// `STRIP_MIN_PX` design pixels.
const STRIP_SHARE: f32 = 0.24;
const STRIP_MIN_PX: f32 = 40.0;

/// One per-member column, in the domain the DSP blends it in.
#[derive(Clone, Debug)]
enum Column {
    Const(f32),
    Blend {
        values: Vec<f32>,
        mean: f32,
        lo: f32,
        hi: f32,
        exaggerate: bool,
        axes: [f32; 3],
        /// Snare form: the axes are added AFTER the exaggerate clip, inside a
        /// clip widened to `(min x lo2, max x hi2)`. Kick form (None): the axes
        /// sit inside the one exaggerate clip.
        relative: Option<(f32, f32)>,
    },
}

impl Column {
    /// The generated dsp.lisp's per-column expression, at lerp factor `t`
    /// (the blend, or the slot's energy-weighted blend).
    fn eval(&self, m: &Morph, t: f32) -> f32 {
        match self {
            Column::Const(value) => *value,
            Column::Blend {
                values,
                mean,
                lo,
                hi,
                exaggerate,
                axes,
                relative,
            } => {
                let a = values.get(m.a).copied().unwrap_or(0.0);
                let b = values.get(m.b).copied().unwrap_or(0.0);
                let blended = a + (b - a) * t;
                if !exaggerate {
                    return blended;
                }
                let shift = axes[0] * m.axes[0] + axes[1] * m.axes[1] + axes[2] * m.axes[2];
                let pushed = blended + m.exaggerate * (blended - mean);
                match relative {
                    None => (pushed + shift).clamp(*lo, *hi),
                    Some((lo2, hi2)) => {
                        let x = pushed.clamp(*lo, *hi);
                        (x + shift).clamp(x.min(*lo2), x.max(*hi2))
                    }
                }
            }
        }
    }
}

/// How a slot's amplitude is exaggerated and moved by the axes.
#[derive(Clone, Copy, Debug)]
enum AmpMode {
    /// Kick: `am * exp(clip(ex log(am / ma) + axes, -4, 4))`.
    Kick { mean: f32 },
    /// Snare: `x = am * exp(clip(ex (log am - lmu), -3, 3))`, then
    /// `min(x exp(axes), max(x cap))`.
    Snare { log_mean: f32, cap: f32 },
}

#[derive(Clone, Debug)]
struct Slot {
    lf0: Column,
    ld0: Column,
    g: Column,
    lt: Column,
    lr: Column,
    t0: Column,
    g2: Option<Column>,
    lt2: Option<Column>,
    /// Snare ring share (0 = body knob, 1 = ring knob); None = body only.
    ring: Option<Column>,
    amp: Vec<f32>,
    amp_mode: AmpMode,
    amp_axes: [f32; 3],
    phase: Vec<f32>,
    /// Per-member energy weight: everything but the amplitude blends with
    /// `pw = (wB bl + 1e-9) / (wA (1 - bl) + wB bl + 2e-9)` instead of `bl`.
    weight: Option<Vec<f32>>,
}

impl Slot {
    fn lerp(&self, m: &Morph) -> f32 {
        match &self.weight {
            Some(w) => {
                let wa = w.get(m.a).copied().unwrap_or(0.0);
                let wb = w.get(m.b).copied().unwrap_or(0.0);
                (wb * m.blend + 1e-9) / (wa * (1.0 - m.blend) + wb * m.blend + 2e-9)
            }
            None => m.blend,
        }
    }

    fn amplitude(&self, m: &Morph) -> f32 {
        let a = self.amp.get(m.a).copied().unwrap_or(0.0);
        let b = self.amp.get(m.b).copied().unwrap_or(0.0);
        let am = a + (b - a) * m.blend;
        let axes = self.amp_axes[0] * m.axes[0] + self.amp_axes[1] * m.axes[1] + self.amp_axes[2] * m.axes[2];
        match self.amp_mode {
            AmpMode::Kick { mean } => {
                am * (m.exaggerate * (am.max(1e-9) / mean).ln() + axes).clamp(-4.0, 4.0).exp()
            }
            AmpMode::Snare { log_mean, cap } => {
                let x = am * (m.exaggerate * (am.max(1e-9).ln() - log_mean)).clamp(-3.0, 3.0).exp();
                (x * axes.exp()).min(x.max(cap))
            }
        }
    }
}

/// Which instrument's voice the modal preview follows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum VoiceKind {
    Kick,
    Snare,
}

/// Real renders of the instrument on a blend lattice, as peak envelopes
/// normalized to each render's peak: `members[i]` is member i alone, and
/// `pairs[(i, j)]` (i < j) holds `steps` envelopes from i (blend 0) to j.
#[derive(Clone, Debug)]
struct Lattice {
    members: Vec<Vec<f32>>,
    pairs: HashMap<(usize, usize), Vec<Vec<f32>>>,
}

#[derive(Clone, Debug)]
enum Preview {
    Modal { voice: VoiceKind, slots: Vec<Slot> },
    Lattice(Lattice),
}

#[derive(Clone, Debug)]
pub struct Family {
    /// Display names, one per member (kick numbers, "SP01", "CL81" ...).
    pub members: Vec<String>,
    pub scores: Vec<[f32; 3]>,
    preview: Preview,
}

/// The morph the DSP is asked for: member indices, blend, exaggerate, axes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Morph {
    a: usize,
    b: usize,
    blend: f32,
    exaggerate: f32,
    axes: [f32; 3],
}

/// Voice knobs the preview applies after the morph (all at their defaults =
/// as fitted). Kick: spread / beat / tilt / drop time. Snare: decay / bend /
/// body / ring / tune.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    spread: f32,
    beat: f32,
    tilt: f32,
    drop_time: f32,
    decay: f32,
    bend: f32,
    body: f32,
    ring: f32,
    tune: f32,
}

impl Default for Voice {
    fn default() -> Self {
        Self { spread: 1.0, beat: 1.0, tilt: 0.0, drop_time: 1.0, decay: 1.0, bend: 1.0, body: 1.0, ring: 1.0, tune: 1.0 }
    }
}

fn json_f32(value: &serde_json::Value) -> Option<f32> {
    value.as_f64().map(|v| v as f32)
}

fn json_vec(value: Option<&serde_json::Value>) -> Option<Vec<f32>> {
    value?.as_array()?.iter().map(json_f32).collect()
}

fn json_axes(value: Option<&serde_json::Value>) -> [f32; 3] {
    let values = json_vec(value).unwrap_or_default();
    std::array::from_fn(|i| values.get(i).copied().unwrap_or(0.0))
}

fn parse_column(value: &serde_json::Value) -> Option<Column> {
    if let Some(constant) = value.get("c") {
        return Some(Column::Const(json_f32(constant)?));
    }
    let relative = json_vec(value.get("rel")).and_then(|r| (r.len() == 2).then(|| (r[0], r[1])));
    Some(Column::Blend {
        values: json_vec(value.get("v"))?,
        mean: value.get("mu").and_then(json_f32).unwrap_or(0.0),
        lo: value.get("lo").and_then(json_f32).unwrap_or(f32::MIN),
        hi: value.get("hi").and_then(json_f32).unwrap_or(f32::MAX),
        exaggerate: value.get("ex").and_then(serde_json::Value::as_bool).unwrap_or(true),
        axes: json_axes(value.get("pc")),
        relative,
    })
}

fn parse_slot(value: &serde_json::Value) -> Option<Slot> {
    let column = |key: &str| value.get(key).and_then(parse_column);
    let amp = value.get("am")?;
    let amp_mode = match (amp.get("lmu").and_then(json_f32), amp.get("cap").and_then(json_f32)) {
        (Some(log_mean), Some(cap)) => AmpMode::Snare { log_mean, cap },
        _ => AmpMode::Kick { mean: amp.get("ma").and_then(json_f32).unwrap_or(1.0).max(1e-9) },
    };
    Some(Slot {
        lf0: column("lf0")?,
        ld0: column("ld0")?,
        g: column("g")?,
        lt: column("lt")?,
        lr: column("lr")?,
        t0: column("t0")?,
        g2: column("g2"),
        lt2: column("lt2"),
        ring: column("rg"),
        amp: json_vec(amp.get("v"))?,
        amp_mode,
        amp_axes: json_axes(amp.get("pc")),
        phase: json_vec(value.get("ph"))?,
        weight: json_vec(value.get("w")),
    })
}

fn parse_lattice(value: &serde_json::Value, n: usize) -> Option<Lattice> {
    let rows = |v: &serde_json::Value| -> Option<Vec<Vec<f32>>> {
        v.as_array()?.iter().map(|row| json_vec(Some(row))).collect()
    };
    let members = rows(value.get("members")?)?;
    let mut pairs = HashMap::new();
    for (key, steps) in value.get("pairs")?.as_object()? {
        let (i, j) = key.split_once('-')?;
        let (i, j): (usize, usize) = (i.parse().ok()?, j.parse().ok()?);
        if i >= j || j >= n {
            return None;
        }
        let steps = rows(steps)?;
        if steps.len() < 2 {
            return None;
        }
        pairs.insert((i, j), steps);
    }
    (members.len() == n && !members.iter().any(Vec::is_empty)).then_some(Lattice { members, pairs })
}

/// Parse a `family.json` document (version 1).
pub fn parse_family(text: &str) -> Option<Family> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let members: Vec<String> = match json.get("names").and_then(serde_json::Value::as_array) {
        Some(names) => names.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>()?,
        None => json
            .get("kicks")
            .or_else(|| json.get("members"))?
            .as_array()?
            .iter()
            .map(|v| v.as_i64().map(|n| n.to_string()))
            .collect::<Option<_>>()?,
    };
    let scores: Vec<[f32; 3]> = json
        .get("scores")?
        .as_array()?
        .iter()
        .map(|row| Some(json_axes(Some(row))))
        .collect::<Option<_>>()?;
    let n = members.len();
    if n == 0 || scores.len() != n {
        return None;
    }
    let preview = if let Some(lattice) = json.get("lattice") {
        Preview::Lattice(parse_lattice(lattice, n)?)
    } else {
        let slots: Vec<Slot> = json
            .get("slots")?
            .as_array()?
            .iter()
            .map(parse_slot)
            .collect::<Option<_>>()?;
        let consistent = slots.iter().all(|slot| {
            slot.amp.len() == n
                && slot.phase.len() == n
                && slot.weight.as_ref().is_none_or(|w| w.len() == n)
                && [&slot.lf0, &slot.ld0, &slot.g, &slot.lt, &slot.lr, &slot.t0]
                    .into_iter()
                    .chain(slot.g2.as_ref())
                    .chain(slot.lt2.as_ref())
                    .chain(slot.ring.as_ref())
                    .all(|column| match column {
                        Column::Const(_) => true,
                        Column::Blend { values, .. } => values.len() == n,
                    })
        });
        if !consistent {
            return None;
        }
        let voice = match json.get("voice").and_then(serde_json::Value::as_str) {
            Some("snare") => VoiceKind::Snare,
            _ => VoiceKind::Kick,
        };
        Preview::Modal { voice, slots }
    };
    Some(Family { members, scores, preview })
}

fn family_cache() -> &'static Mutex<HashMap<String, Option<Arc<Family>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Family>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load (once per path) the family sidecar, resolved against the same
/// factory/user asset roots as `wavetable-viewer` banks.
pub fn load_family(path: &str) -> Option<Arc<Family>> {
    if let Some(cached) = family_cache().lock().ok()?.get(path) {
        return cached.clone();
    }
    let raw = std::path::Path::new(path);
    let resolved = if raw.is_absolute() {
        Some(raw.to_path_buf())
    } else {
        super::patcher::resolve_asset_reference(path, Some(std::path::Path::new(".")))
    };
    let loaded = resolved
        .and_then(|resolved| std::fs::read_to_string(resolved).ok())
        .and_then(|text| parse_family(&text))
        .map(Arc::new);
    if loaded.is_none() {
        eprintln!("[family-map] failed to load family: {path}");
    }
    family_cache()
        .lock()
        .ok()?
        .insert(path.to_string(), loaded.clone());
    loaded
}

/// Wrap a phase difference into (-pi, pi]: the DSP's shortest-arc blend.
fn wrap_phase(delta: f32) -> f32 {
    delta - TAU * (delta / TAU + 0.5).floor()
}

/// The morphed tonal bank, sampled at `PREVIEW_RATE` and normalized to its
/// peak, following each voice's dsp.lisp:
/// - kick: `lf` spread / beat about the sub, `idec` tilt, `tau` drop time, the
///   glide `sh = max(-0.9 f, g f)` and its second stage;
/// - snare: `amp (body + rg (ring - body))`, `idec / decay`, the glide
///   `max(-0.9 f, bend g f)`, pitch times tune.
///
/// Both: the attack `1 - e^(-t irise)` and the per-slot amplitude.
fn preview_samples(voice_kind: VoiceKind, slots: &[Slot], morph: &Morph, voice: &Voice) -> Vec<f32> {
    let count = (PREVIEW_SECONDS * PREVIEW_RATE) as usize;
    let mut out = vec![0.0f32; count];
    let lf_sub = slots.first().map(|slot| slot.lf0.eval(morph, slot.lerp(morph))).unwrap_or(0.0);
    let snare = voice_kind == VoiceKind::Snare;
    for (j, slot) in slots.iter().enumerate() {
        let t = slot.lerp(morph);
        let lf0 = slot.lf0.eval(morph, t);
        let lf = match (snare, j) {
            (true, _) | (false, 0) => lf0,
            (false, 1) => lf_sub + voice.beat.clamp(0.0, 4.0) * (lf0 - lf_sub),
            (false, _) => lf_sub + voice.spread.clamp(0.5, 1.6) * (lf0 - lf_sub),
        };
        let f = lf.exp();
        let pitch = if snare { voice.tune.clamp(0.5, 2.0) } else { 1.0 };
        if !(f.is_finite() && f > 0.0 && f * pitch < PREVIEW_FMAX) {
            continue;
        }
        let mut amp = slot.amplitude(morph);
        if snare {
            let body = voice.body.clamp(0.0, 2.0);
            let rg = slot.ring.as_ref().map_or(0.0, |ring| ring.eval(morph, t));
            amp *= body + rg * (voice.ring.clamp(0.0, 3.0) - body);
        }
        if amp.abs() < 1e-7 {
            continue;
        }
        let ld0 = slot.ld0.eval(morph, t);
        let idec = if snare {
            (-ld0).exp() / voice.decay.clamp(0.25, 3.0)
        } else {
            (-(ld0 + voice.tilt.clamp(-2.0, 2.0) * (lf - lf_sub))).exp()
        };
        let drop_time = if snare { 1.0 } else { voice.drop_time.clamp(0.25, 4.0) };
        let bend = if snare { voice.bend.clamp(0.0, 2.0) } else { 1.0 };
        let tau = drop_time * slot.lt.eval(morph, t).exp();
        let irise = (-slot.lr.eval(morph, t)).exp();
        let t0 = slot.t0.eval(morph, t);
        let sh = (bend * slot.g.eval(morph, t) * f).max(-0.9 * f);
        let (sh2, tau2) = match (&slot.g2, &slot.lt2) {
            (Some(g2), Some(lt2)) => ((g2.eval(morph, t) * f).max(-0.9 * f), drop_time * lt2.eval(morph, t).exp()),
            _ => (0.0, 1.0),
        };
        let pa = slot.phase.get(morph.a).copied().unwrap_or(0.0);
        let pb = slot.phase.get(morph.b).copied().unwrap_or(0.0);
        let phase = pa + t * wrap_phase(pb - pa);
        for (i, sample) in out.iter_mut().enumerate() {
            let time = (i as f32 / PREVIEW_RATE - t0).max(0.0);
            let envelope = amp * (-time * idec).exp() * (1.0 - (-time * irise).exp());
            let glide = sh * tau * (1.0 - (-time / tau).exp()) + sh2 * tau2 * (1.0 - (-time / tau2).exp());
            *sample += envelope * (TAU * pitch * (f * time + glide) + phase).sin();
        }
    }
    let peak = out.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
    if peak > 1e-12 {
        out.iter_mut().for_each(|v| *v /= peak);
    }
    out
}

/// The lattice envelope at this morph: member A alone when A == B, else the
/// pair's two nearest blend steps interpolated.
fn lattice_envelope(lattice: &Lattice, morph: &Morph) -> Vec<f32> {
    if morph.a == morph.b {
        return lattice.members.get(morph.a).cloned().unwrap_or_default();
    }
    let (lo, hi, t) = if morph.a < morph.b {
        (morph.a, morph.b, morph.blend)
    } else {
        (morph.b, morph.a, 1.0 - morph.blend)
    };
    let Some(steps) = lattice.pairs.get(&(lo, hi)) else {
        // No renders for this pair: fall back to the endpoints' crossfade.
        let (a, b) = (&lattice.members[lo], &lattice.members[hi]);
        return a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect();
    };
    let position = t.clamp(0.0, 1.0) * (steps.len() - 1) as f32;
    let i = (position.floor() as usize).min(steps.len() - 2);
    let frac = position - i as f32;
    steps[i].iter().zip(&steps[i + 1]).map(|(x, y)| x + (y - x) * frac).collect()
}

/// What the preview strip draws: a signed waveform (modal) or an envelope.
enum PreviewTrace {
    Wave(Rc<Vec<f32>>),
    Envelope(Vec<f32>),
}

#[derive(Default)]
struct PreviewCache {
    key: Option<(usize, [u32; 16])>,
    samples: Rc<Vec<f32>>,
}

thread_local! {
    static PREVIEW: RefCell<PreviewCache> = RefCell::new(PreviewCache::default());
}

fn preview_trace(family: &Arc<Family>, morph: &Morph, voice: &Voice) -> PreviewTrace {
    let (kind, slots) = match &family.preview {
        Preview::Lattice(lattice) => return PreviewTrace::Envelope(lattice_envelope(lattice, morph)),
        Preview::Modal { voice, slots } => (*voice, slots),
    };
    let bits = [
        morph.a as f32,
        morph.b as f32,
        morph.blend,
        morph.exaggerate,
        morph.axes[0],
        morph.axes[1],
        morph.axes[2],
        voice.spread,
        voice.beat,
        voice.tilt,
        voice.drop_time,
        voice.decay,
        voice.bend,
        voice.body,
        voice.ring,
        voice.tune,
    ]
    .map(f32::to_bits);
    let key = (Arc::as_ptr(family) as usize, bits);
    PreviewTrace::Wave(PREVIEW.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.key != Some(key) {
            cache.samples = Rc::new(preview_samples(kind, slots, morph, voice));
            cache.key = Some(key);
        }
        cache.samples.clone()
    }))
}

fn member_index(props: &HashMap<String, Value>, key: &str, count: usize) -> usize {
    // dsp.lisp: kiA = floor(clip(kick_a, 0, N-1) + 1.5), 1-based.
    let raw = get_f32_prop(props, key, 0.0).clamp(0.0, count.saturating_sub(1) as f32);
    ((raw + 0.5).floor() as usize).min(count.saturating_sub(1))
}

fn morph_from_props(props: &HashMap<String, Value>, count: usize) -> Morph {
    Morph {
        a: member_index(props, "a", count),
        b: member_index(props, "b", count),
        blend: get_f32_prop(props, "blend", 0.0).clamp(0.0, 1.0),
        exaggerate: get_f32_prop(props, "exaggerate", 0.0).clamp(-1.0, 2.0),
        axes: [
            get_f32_prop(props, "pc1", 0.0).clamp(-3.0, 3.0),
            get_f32_prop(props, "pc2", 0.0).clamp(-3.0, 3.0),
            get_f32_prop(props, "pc3", 0.0).clamp(-3.0, 3.0),
        ],
    }
}

fn voice_from_props(props: &HashMap<String, Value>) -> Voice {
    let d = Voice::default();
    Voice {
        spread: get_f32_prop(props, "spread", d.spread),
        beat: get_f32_prop(props, "beat", d.beat),
        tilt: get_f32_prop(props, "tilt", d.tilt),
        drop_time: get_f32_prop(props, "drop-time", d.drop_time),
        decay: get_f32_prop(props, "decay", d.decay),
        bend: get_f32_prop(props, "bend", d.bend),
        body: get_f32_prop(props, "body", d.body),
        ring: get_f32_prop(props, "ring", d.ring),
        tune: get_f32_prop(props, "tune", d.tune),
    }
}

fn family_from_props(props: &HashMap<String, Value>) -> Option<Arc<Family>> {
    match props.get("file")? {
        Value::String(path) => load_family(path),
        _ => None,
    }
}

/// Plot geometry: the centre and the cell size of one standard deviation on
/// each axis. Each axis is fitted to the family on its own, so a wide display
/// spreads the members out instead of leaving the sides empty.
struct Plot {
    center: [f32; 2],
    sd: [f32; 2],
    rect: Rect,
}

impl Plot {
    fn new(rect: Rect, family: &Family) -> Self {
        let extent = |k: usize| {
            family
                .scores
                .iter()
                .fold(0.0f32, |m, score| m.max(score[k].abs()))
                .max(MIN_EXTENT_SD / EXTENT_MARGIN)
                * EXTENT_MARGIN
        };
        Self {
            center: [rect.col + rect.width * 0.5, rect.row + rect.height * 0.5],
            sd: [rect.width * 0.5 / extent(0), rect.height * 0.5 / extent(1)],
            rect,
        }
    }

    /// Axis 1 to the right, axis 2 up, clamped inside the rect.
    fn point(&self, x: f32, y: f32) -> [f32; 2] {
        let margin_x = self.sd[0] * 0.15;
        let margin_y = self.sd[1] * 0.15;
        [
            (self.center[0] + x * self.sd[0])
                .clamp(self.rect.col + margin_x, self.rect.col + self.rect.width - margin_x),
            (self.center[1] - y * self.sd[1])
                .clamp(self.rect.row + margin_y, self.rect.row + self.rect.height - margin_y),
        ]
    }
}

/// Split the widget into the plot (top) and the preview strip (bottom).
fn split_rect(rect: Rect, cell_h: f32) -> (Rect, Rect) {
    let strip_h = (super::ui_design_px(STRIP_MIN_PX) / cell_h.max(1.0))
        .max(rect.height * STRIP_SHARE)
        .min(rect.height * 0.45);
    let plot = Rect { height: rect.height - strip_h, ..rect };
    let strip = Rect { row: rect.row + rect.height - strip_h, height: strip_h, ..rect };
    (plot, strip)
}

fn dot_radius_px(axis3: f32) -> f32 {
    2.5 + (axis3.clamp(-3.0, 3.0) + 3.0) * 0.7
}

/// The blend point and where Exaggerate + the axis offsets take it.
fn morph_points(family: &Family, morph: &Morph) -> ([f32; 3], [f32; 3]) {
    let sa = family.scores[morph.a];
    let sb = family.scores[morph.b];
    let blend: [f32; 3] = std::array::from_fn(|k| sa[k] + (sb[k] - sa[k]) * morph.blend);
    // Exaggerate is linear about the family mean (score 0), the axes add.
    let moved: [f32; 3] = std::array::from_fn(|k| blend[k] * (1.0 + morph.exaggerate) + morph.axes[k]);
    (blend, moved)
}

fn nearest_member(
    family: &Family,
    plot: &Plot,
    col: f32,
    row: f32,
    cell_w: f32,
    cell_h: f32,
) -> Option<usize> {
    let limit = super::ui_design_px(PICK_RADIUS_PX);
    family
        .scores
        .iter()
        .enumerate()
        .map(|(i, score)| {
            let p = plot.point(score[0], score[1]);
            let dx = (p[0] - col) * cell_w;
            let dy = (p[1] - row) * cell_h;
            (i, dx.hypot(dy))
        })
        .filter(|(_, distance)| *distance <= limit)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn pick_event(index: usize, slot: usize) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(vec![
        Rc::new(RefCell::new(Value::Number(index as f64))),
        Rc::new(RefCell::new(Value::Number(slot as f64))),
    ]))
}

/// `color` mixed over `under` at `amount`, opaque. Text is drawn without
/// alpha blending, so dim labels are mixed against the plot background.
fn mixed(under: Color, color: Color, amount: f32) -> Color {
    Color::rgba(
        under.r + (color.r - under.r) * amount,
        under.g + (color.g - under.g) * amount,
        under.b + (color.b - under.b) * amount,
        1.0,
    )
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha,
        ..color
    }
}

fn text(row: f32, col: f32, label: String, size: f32, fg: Color) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
        row,
        col,
        align_width: 0.0,
        h_align: 0.0,
        text: label,
        font_size: size,
        scale: 1.0,
        fg,
        bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
        mono: false,
    })
}

/// Closed polyline approximating a circle of `radius_px` design pixels.
fn ring_points(center: [f32; 2], radius_px: f32, viewport: WidgetViewport) -> Vec<[f32; 2]> {
    let r = super::ui_design_px(radius_px);
    let rx = r / viewport.cell_w.max(1.0);
    let ry = r / viewport.cell_h.max(1.0);
    (0..=40)
        .map(|i| {
            let a = i as f32 / 40.0 * TAU;
            [center[0] + rx * a.cos(), center[1] + ry * a.sin()]
        })
        .collect()
}

/// Dash a straight segment into `dash_px` on / `gap_px` off pieces.
fn dashed(mesh: &mut ShadedMesh, a: [f32; 2], b: [f32; 2], color: Color, viewport: WidgetViewport) {
    let cell_w = viewport.cell_w.max(1.0);
    let cell_h = viewport.cell_h.max(1.0);
    let length_px = ((b[0] - a[0]) * cell_w).hypot((b[1] - a[1]) * cell_h);
    let dash = super::ui_design_px(3.0);
    let gap = super::ui_design_px(3.0);
    if length_px < 1.0 {
        return;
    }
    let mut start = 0.0;
    while start < length_px {
        let end = (start + dash).min(length_px);
        let p = |d: f32| [a[0] + (b[0] - a[0]) * d / length_px, a[1] + (b[1] - a[1]) * d / length_px];
        mesh.push_polyline(&[p(start), p(end)], color, viewport, 0.5);
        start = end + gap;
    }
}

impl WidgetDefinition for FamilyMapWidget {
    fn names(&self) -> &'static [&'static str] {
        &["family-map"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "a",
            "b",
            "blend",
            "exaggerate",
            "pc1",
            "pc2",
            "pc3",
            "spread",
            "beat",
            "tilt",
            "drop-time",
            "decay",
            "bend",
            "body",
            "ring",
            "tune",
            "armed",
        ]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "file",
            "a",
            "b",
            "blend",
            "exaggerate",
            "pc1",
            "pc2",
            "pc3",
            "spread",
            "beat",
            "tilt",
            "drop-time",
            "decay",
            "bend",
            "body",
            "ring",
            "tune",
            "armed",
            "accent",
            "b-color",
            "dot-color",
            "line-color",
            "grid-color",
            "label-color",
            "background-color",
            "inset-color",
            "x-label",
            "y-label",
            "width",
            "height",
            "on-pick",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width)
                .max(1.0),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(8.0)
                .max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let Some(family) = family_from_props(props) else {
            return;
        };
        let width = rect.width.max(1.0);
        let height = rect.height.max(1.0);
        let n = family.members.len();
        let morph = morph_from_props(props, n);
        let plot = Plot::new(Rect { col: 0.0, row: 0.0, width: width - 1.0, height: height - 1.0 }, &family);
        for (i, score) in family.scores.iter().enumerate() {
            let [col, row] = plot.point(score[0], score[1]);
            let (ch, fg) = if i == morph.a {
                ('A', resolve_named_color(props, "accent", theme::WIDGET_KNOB_FILLED()))
            } else if i == morph.b {
                ('B', theme::FG())
            } else {
                ('·', theme::WIDGET_KNOB_TRACK())
            };
            buf.set(
                (rect.row + row).round() as u16,
                (rect.col + col).round() as u16,
                styled_cell(ch, fg, None),
            );
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        cell_w: f32,
        cell_h: f32,
    ) -> MouseEventOutcome {
        let MouseEventKind::Down(MouseButton::Left) = mouse_kind else {
            return match mouse_kind {
                MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
                _ => MouseEventOutcome::Ignore,
            };
        };
        let Some(family) = family_from_props(&node.props) else {
            return MouseEventOutcome::Ignore;
        };
        let plot = Plot::new(split_rect(node.rect, cell_h).0, &family);
        match nearest_member(&family, &plot, local_col, local_row, cell_w.max(1.0), cell_h.max(1.0)) {
            Some(index) => {
                let armed = usize::from(get_f32_prop(&node.props, "armed", 0.0) > 0.5);
                let slot = if modifiers.contains(KeyModifiers::SHIFT) { 1 - armed } else { armed };
                MouseEventOutcome::Dispatch(pick_event(index, slot))
            }
            None => MouseEventOutcome::Consume,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::List(items)) = event else {
            return None;
        };
        let callback = node.props.get("on-pick")?.clone();
        let args = items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::Number(number) => Value::Number(*number),
                _ => Value::Number(0.0),
            })
            .collect();
        Some(EventOutput { callback, args })
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let rect = node.rect;
        if !(rect.width.is_finite() && rect.height.is_finite())
            || rect.width <= 0.0
            || rect.height <= 0.0
        {
            return Vec::new();
        }
        let props = &node.props;
        let background = resolve_named_color(props, "background-color", theme::WIDGET_KNOB_TRACK());
        let mut prims = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect,
            color: background,
        })];
        let Some(family) = family_from_props(props) else {
            return prims;
        };
        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let accent = resolve_named_color(props, "accent", theme::WIDGET_KNOB_FILLED());
        let b_color = resolve_named_color(props, "b-color", theme::FG());
        let dot_color = resolve_named_color(props, "dot-color", with_alpha(theme::FG(), 0.38));
        let line_color = resolve_named_color(props, "line-color", with_alpha(theme::FG(), 0.22));
        let grid_color = resolve_named_color(props, "grid-color", with_alpha(theme::FG(), 0.06));
        let label_color = resolve_named_color(props, "label-color", mixed(background, theme::FG(), 0.42));
        let inset_color = resolve_named_color(props, "inset-color", with_alpha(theme::FG(), 0.05));
        let morph = morph_from_props(props, family.members.len());
        let (plot_rect, strip) = split_rect(rect, cell_h);
        let plot = Plot::new(plot_rect, &family);
        let hair_w = super::ui_design_px(1.0) / cell_w;
        let hair_h = super::ui_design_px(1.0) / cell_h;

        // Grid: one line per standard deviation, the axes a touch brighter.
        let steps = (plot_rect.width / plot.sd[0]).max(plot_rect.height / plot.sd[1]).ceil() as i32;
        for k in -steps..=steps {
            let color = if k == 0 { with_alpha(grid_color, 2.2) } else { grid_color };
            let x = plot.center[0] + k as f32 * plot.sd[0];
            if x > plot_rect.col && x < plot_rect.col + plot_rect.width {
                prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect { col: x - hair_w * 0.5, width: hair_w, ..plot_rect },
                    color,
                }));
            }
            let y = plot.center[1] + k as f32 * plot.sd[1];
            if y > plot_rect.row && y < plot_rect.row + plot_rect.height {
                prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect { row: y - hair_h * 0.5, height: hair_h, ..plot_rect },
                    color,
                }));
            }
        }
        let pad_w = super::ui_design_px(8.0) / cell_w;
        let pad_h = super::ui_design_px(6.0) / cell_h;
        if let Some(Value::String(x_label)) = props.get("x-label") {
            let chars = x_label.chars().count() as f32;
            prims.push(text(
                plot.center[1] - 1.0 - pad_h * 0.2,
                plot_rect.col + plot_rect.width - pad_w - chars * 0.62,
                x_label.clone(),
                9.0,
                label_color,
            ));
        }
        if let Some(Value::String(y_label)) = props.get("y-label") {
            prims.push(text(plot_rect.row + pad_h * 0.4, plot.center[0] + pad_w * 0.6, y_label.clone(), 9.0, label_color));
        }

        let mut mesh = ShadedMesh::new();
        let pa = plot.point(family.scores[morph.a][0], family.scores[morph.a][1]);
        let pb = plot.point(family.scores[morph.b][0], family.scores[morph.b][1]);
        if morph.a != morph.b {
            mesh.push_polyline(&[pa, pb], line_color, viewport, 0.5);
        }
        // Members, back to front: the rest, then B, then A on top.
        let mut order: Vec<usize> = (0..family.members.len())
            .filter(|&i| i != morph.a && i != morph.b)
            .collect();
        order.push(morph.b);
        order.push(morph.a);
        for i in order {
            let score = family.scores[i];
            let color = if i == morph.a {
                accent
            } else if i == morph.b {
                b_color
            } else {
                dot_color
            };
            mesh.push_disc(plot.point(score[0], score[1]), dot_radius_px(score[2]), color, viewport);
        }
        let (blend, moved) = morph_points(&family, &morph);
        let p_blend = plot.point(blend[0], blend[1]);
        let p_moved = plot.point(moved[0], moved[1]);
        dashed(&mut mesh, p_blend, p_moved, accent, viewport);
        mesh.push_disc(p_blend, 3.0, b_color, viewport);
        let ring = 5.0 + (moved[2].clamp(-3.0, 3.0) + 3.0) * 1.2;
        mesh.push_polyline(&ring_points(p_moved, ring, viewport), accent, viewport, 0.75);

        // Morph preview strip under the plot, set off by a hairline.
        let inset = strip;
        let inset_w = inset.width;
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: inset,
            color: inset_color,
        }));
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect { height: hair_h, ..inset },
            color: with_alpha(grid_color, 2.2),
        }));
        let x0 = inset.col + pad_w;
        let span = inset_w - pad_w * 2.0;
        let columns = ((span * cell_w / super::ui_design_px(1.0)) as usize).clamp(16, 600);
        let mid = inset.row + inset.height * 0.5;
        let half = inset.height * 0.40;
        let column_x = |c: usize| x0 + span * c as f32 / (columns - 1).max(1) as f32;
        match preview_trace(&family, &morph, &voice_from_props(props)) {
            PreviewTrace::Wave(samples) if !samples.is_empty() => {
                // Min/max per pixel column, in time order, so dense cycles fill
                // in rather than aliasing into a moire.
                let mut points = Vec::with_capacity(columns * 2);
                let per = samples.len() as f32 / columns as f32;
                for c in 0..columns {
                    let start = ((c as f32 * per) as usize).min(samples.len() - 1);
                    let end = (((c + 1) as f32 * per) as usize).min(samples.len()).max(start + 1);
                    let chunk = &samples[start..end];
                    let (lo_i, lo) = chunk.iter().enumerate().fold((0, f32::MAX), |acc, (i, v)| if *v < acc.1 { (i, *v) } else { acc });
                    let (hi_i, hi) = chunk.iter().enumerate().fold((0, f32::MIN), |acc, (i, v)| if *v > acc.1 { (i, *v) } else { acc });
                    let (first, second) = if lo_i <= hi_i { (lo, hi) } else { (hi, lo) };
                    points.push([column_x(c), mid - first * half]);
                    points.push([column_x(c), mid - second * half]);
                }
                mesh.push_polyline(&points, with_alpha(b_color, 0.85), viewport, 0.45);
            }
            PreviewTrace::Envelope(envelope) if !envelope.is_empty() => {
                // A rendered hit's peak envelope, mirrored: filled columns plus
                // an outline top and bottom.
                let at = |c: usize| {
                    let position = c as f32 / (columns - 1).max(1) as f32 * (envelope.len() - 1) as f32;
                    let i = (position.floor() as usize).min(envelope.len() - 1);
                    let j = (i + 1).min(envelope.len() - 1);
                    let frac = position - i as f32;
                    (envelope[i] + (envelope[j] - envelope[i]) * frac).clamp(0.0, 1.0)
                };
                let fill = with_alpha(b_color, 0.22);
                let column_w = span / columns as f32;
                let mut top = Vec::with_capacity(columns);
                let mut bottom = Vec::with_capacity(columns);
                for c in 0..columns {
                    let e = at(c) * half;
                    let x = column_x(c);
                    prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: Rect { col: x - column_w * 0.5, row: mid - e, width: column_w, height: (e * 2.0).max(hair_h) },
                        color: fill,
                    }));
                    top.push([x, mid - e]);
                    bottom.push([x, mid + e]);
                }
                mesh.push_polyline(&top, with_alpha(b_color, 0.85), viewport, 0.45);
                mesh.push_polyline(&bottom, with_alpha(b_color, 0.85), viewport, 0.45);
            }
            _ => {}
        }
        mesh.push_into(&mut prims);

        // Tags for A and B, right of their dots, or left of them when the
        // name would run past the plot's right edge.
        let tag = |p: [f32; 2], label: String, color: Color| {
            let gap = super::ui_design_px(13.0) / cell_w;
            let width = label.chars().count() as f32 * 0.62;
            if p[0] + gap + width > plot_rect.col + plot_rect.width - pad_w {
                let mut primitive = text(p[1] - 0.5, p[0] - gap - width, label, 9.5, color);
                if let GpuPrimitive::ProportionalText(t) = &mut primitive {
                    t.align_width = width;
                    t.h_align = 1.0;
                }
                primitive
            } else {
                text(p[1] - 0.5, p[0] + gap, label, 9.5, color)
            }
        };
        if morph.a == morph.b {
            prims.push(tag(pa, format!("ab {}", family.members[morph.a]), accent));
        } else {
            prims.push(tag(pb, format!("b {}", family.members[morph.b]), b_color));
            prims.push(tag(pa, format!("a {}", family.members[morph.a]), accent));
        }
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"version":1,"kicks":[51,52],"scores":[[-1,0.5,0],[2,-1,1]],
      "slots":[
        {"lf0":{"v":[3.9,4.1],"mu":4.0,"lo":0,"hi":9,"ex":true,"pc":[0.1,0,0]},
         "ld0":{"v":[-2.5,-2.0],"mu":-2.25,"lo":-9,"hi":3,"ex":true,"pc":[0,0,0]},
         "g":{"v":[2.0,1.0],"mu":1.5,"lo":-0.95,"hi":14,"ex":true,"pc":[0,0,0]},
         "lt":{"v":[-3.5,-3.0],"mu":-3.25,"lo":-9,"hi":1,"ex":true,"pc":[0,0,0]},
         "lr":{"v":[-6,-6],"mu":-6,"lo":-9,"hi":1,"ex":true,"pc":[0,0,0]},
         "t0":{"c":0},
         "am":{"v":[1.0,0.5],"ma":0.75,"pc":[0,0,0]},
         "ph":[0,3.0]}]}"#;

    fn modal(family: &Family) -> (VoiceKind, &[Slot]) {
        match &family.preview {
            Preview::Modal { voice, slots } => (*voice, slots),
            Preview::Lattice(_) => panic!("expected a modal family"),
        }
    }

    #[test]
    fn parses_a_family_sidecar() {
        let family = parse_family(DOC).expect("parses");
        assert_eq!(family.members, vec!["51", "52"]);
        assert_eq!(family.scores[1], [2.0, -1.0, 1.0]);
        assert_eq!(modal(&family).1.len(), 1);
        assert_eq!(modal(&family).0, VoiceKind::Kick);
        assert!(parse_family(r#"{"kicks":[1],"scores":[],"slots":[]}"#).is_none(), "scores must match members");
    }

    #[test]
    fn snare_columns_clip_the_axes_relative_to_the_exaggerated_value() {
        // (def x (clip (+ m (* ex (- m mu))) lo hi))
        // (def v (clip (+ x axes) (min x lo2) (max x hi2)))
        let column = Column::Blend {
            values: vec![1.0, 5.0],
            mean: 3.0,
            lo: 0.0,
            hi: 10.0,
            exaggerate: true,
            axes: [1.0, 0.0, 0.0],
            relative: Some((0.5, 4.5)),
        };
        let at = |axis, t| column.eval(&Morph { a: 0, b: 1, blend: t, exaggerate: 0.0, axes: [axis, 0.0, 0.0] }, t);
        assert_eq!(at(0.0, 0.5), 3.0);
        assert_eq!(at(1.0, 0.5), 4.0, "axes add inside the family range");
        assert_eq!(at(3.0, 0.5), 4.5, "but stop at its edge");
        assert_eq!(at(3.0, 1.0), 5.0, "and never push a value already past the edge further out");
    }

    #[test]
    fn snare_amplitude_and_energy_weights_follow_the_dsp() {
        let slot = Slot {
            lf0: Column::Const(5.0),
            ld0: Column::Const(-2.0),
            g: Column::Const(0.0),
            lt: Column::Const(-3.0),
            lr: Column::Const(-6.0),
            t0: Column::Const(0.0),
            g2: None,
            lt2: None,
            ring: None,
            amp: vec![1.0, 2.0],
            amp_mode: AmpMode::Snare { log_mean: 0.0, cap: 2.5 },
            amp_axes: [1.0, 0.0, 0.0],
            phase: vec![0.0, 0.0],
            weight: Some(vec![1.0, 3.0]),
        };
        let morph = |blend, exaggerate, axis| Morph { a: 0, b: 1, blend, exaggerate, axes: [axis, 0.0, 0.0] };
        // pw = (wB bl + 1e-9) / (wA (1 - bl) + wB bl + 2e-9): 1.5 / 2.0 at bl 0.5.
        assert!((slot.lerp(&morph(0.5, 0.0, 0.0)) - 0.75).abs() < 1e-6);
        assert!((slot.amplitude(&morph(0.5, 0.0, 0.0)) - 1.5).abs() < 1e-6, "amplitude blends with bl, not pw");
        assert!((slot.amplitude(&morph(1.0, 0.0, 1.0)) - 2.5).abs() < 1e-6, "axes capped at max(x, cap)");
        let pushed = slot.amplitude(&morph(1.0, 1.0, 0.0));
        assert!((pushed - 2.0 * 2.0f32.ln().exp()).abs() < 1e-5, "x = am exp(ex (log am - lmu))");
    }

    #[test]
    fn lattice_interpolates_between_rendered_steps() {
        let doc = r#"{"names":["CL81","OP54"],"scores":[[0,0,0],[1,1,0]],
          "lattice":{"members":[[1,0.5],[1,0.9]],"pairs":{"0-1":[[1,0.5],[1,0.6],[1,0.9]]}}}"#;
        let family = parse_family(doc).expect("lattice parses");
        assert_eq!(family.members, vec!["CL81", "OP54"]);
        let Preview::Lattice(lattice) = &family.preview else { panic!("lattice") };
        let at = |a, b, blend| lattice_envelope(lattice, &Morph { a, b, blend, exaggerate: 0.0, axes: [0.0; 3] });
        assert_eq!(at(0, 0, 0.7), vec![1.0, 0.5], "A alone is its own render");
        assert_eq!(at(0, 1, 0.5), vec![1.0, 0.6], "the middle step is a real render");
        assert!((at(0, 1, 0.75)[1] - 0.75).abs() < 1e-6, "between steps: interpolated");
        assert!((at(1, 0, 0.25)[1] - 0.75).abs() < 1e-6, "B->A walks the same renders backwards");
    }

    #[test]
    fn columns_follow_the_dsp_expression() {
        let column = Column::Blend {
            values: vec![1.0, 3.0],
            mean: 1.0,
            lo: -10.0,
            hi: 4.0,
            exaggerate: true,
            axes: [0.5, 0.0, -1.0],
            relative: None,
        };
        let morph = |blend, exaggerate, axes| Morph { a: 0, b: 1, blend, exaggerate, axes };
        let eval = |m: Morph| column.eval(&m, m.blend);
        assert_eq!(eval(morph(0.0, 0.0, [0.0; 3])), 1.0, "A as fitted");
        assert_eq!(eval(morph(0.5, 0.0, [0.0; 3])), 2.0, "lerp");
        assert_eq!(eval(morph(0.5, 1.0, [0.0; 3])), 3.0, "m + ex (m - mu)");
        assert_eq!(eval(morph(0.5, 0.0, [2.0, 0.0, 1.0])), 2.0, "axes add: 2 + 1 - 1");
        assert_eq!(eval(morph(1.0, 2.0, [0.0; 3])), 4.0, "clipped to hi");
    }

    #[test]
    fn preview_is_normalized_and_moves_with_the_blend() {
        let family = parse_family(DOC).unwrap();
        let voice = Voice::default();
        let (kind, slots) = modal(&family);
        let at = |blend| preview_samples(kind, slots, &Morph { a: 0, b: 1, blend, exaggerate: 0.0, axes: [0.0; 3] }, &voice);
        let a = at(0.0);
        let b = at(1.0);
        let peak = a.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        assert!((peak - 1.0).abs() < 1e-5, "normalized to its peak");
        assert_eq!(a.len(), (PREVIEW_SECONDS * PREVIEW_RATE) as usize);
        let differs = a.iter().zip(&b).any(|(x, y)| (x - y).abs() > 1e-3);
        assert!(differs, "a different member draws a different hit");
    }

    #[test]
    fn exaggerate_scales_about_the_mean_and_axes_offset() {
        let family = parse_family(DOC).unwrap();
        let morph = Morph { a: 0, b: 1, blend: 0.5, exaggerate: 1.0, axes: [0.5, 0.0, 0.0] };
        let (blend, moved) = morph_points(&family, &morph);
        assert_eq!(blend, [0.5, -0.25, 0.5]);
        assert_eq!(moved, [1.5, -0.5, 1.0]);
    }

    #[test]
    fn member_index_matches_the_dsp_rounding() {
        let mut props = HashMap::new();
        props.insert("a".to_string(), Value::Number(0.49));
        props.insert("b".to_string(), Value::Number(0.5));
        assert_eq!(member_index(&props, "a", 33), 0);
        assert_eq!(member_index(&props, "b", 33), 1);
        props.insert("a".to_string(), Value::Number(99.0));
        assert_eq!(member_index(&props, "a", 33), 32);
    }

    #[test]
    fn clicking_a_dot_picks_it_into_the_armed_slot() {
        let path = std::env::temp_dir().join(format!("family-map-test-{}.json", std::process::id()));
        std::fs::write(&path, DOC).unwrap();
        let mut props = HashMap::new();
        props.insert("file".to_string(), Value::String(path.to_string_lossy().into_owned()));
        props.insert("on-pick".to_string(), Value::Keyword("cb".to_string()));
        props.insert("armed".to_string(), Value::Number(1.0));
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "family-map".to_string(),
            rect: Rect { col: 0.0, row: 0.0, width: 40.0, height: 20.0 },
            props,
            children: Vec::new(),
            focusable: false,
            animation: crate::layout::LayoutAnimationHints::default(),
        };
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let p = plot.point(family.scores[1][0], family.scores[1][1]);
        let pick = |modifiers| {
            match FAMILY_MAP_WIDGET.mouse_event(&node, MouseEventKind::Down(MouseButton::Left), p[0], p[1], None, None, modifiers, 8.0, 16.0) {
                MouseEventOutcome::Dispatch(event) => FAMILY_MAP_WIDGET.handle_event(&node, event).unwrap().args,
                _ => panic!("a click on a dot dispatches"),
            }
        };
        let numbers = |args: Vec<Value>| args.iter().map(|v| match v { Value::Number(n) => *n, _ => -1.0 }).collect::<Vec<_>>();
        assert_eq!(numbers(pick(KeyModifiers::NONE)), vec![1.0, 1.0], "armed B");
        assert_eq!(numbers(pick(KeyModifiers::SHIFT)), vec![1.0, 0.0], "shift flips to A");
        let miss = FAMILY_MAP_WIDGET.mouse_event(&node, MouseEventKind::Down(MouseButton::Left), 0.2, 0.2, None, None, KeyModifiers::NONE, 8.0, 16.0);
        assert!(matches!(miss, MouseEventOutcome::Consume), "empty space picks nothing");
        let _ = std::fs::remove_file(path);
    }
}
