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
//!
//! Latent mode (for a decoder whose latent dims ARE the axes): an `:on-move`
//! callback turns the plot into a continuous 2D control. A press on empty
//! space, and any drag, dispatches `(on-move x y phase)` in score units
//! (clamped to -3..3), `phase` 0 on press (or on first leaving a pressed dot),
//! 1 while dragging, 2 on release; a
//! press and release on a dot that moves less than `CLICK_SLOP_PX` still picks
//! it. `:x` / `:y` / `:size` place the ring directly at that point (with a
//! dashed lead from member A) instead of deriving it from the morph. Without
//! `:on-move` the widget behaves exactly as before.
//!
//! Markers (latent mode with several points, e.g. a morph's START and END):
//! `:markers` is a list of `(dict :id n :x .. :y .. :size .. :label "START"
//! :near i :color c)`. Each draws as its own labelled ring (the first in
//! `:accent`, the rest in `:b-color`, unless `:color` is given), a faint line
//! joins them in list order (the morph path), and `:selected` (an id; default
//! the first marker) is drawn heavier. A press on a ring grabs that marker
//! (no jump: the grab offset is kept); a press anywhere else moves the
//! selected one there. `on-move` then gets `(x y phase id)`. A short click on
//! a dot dispatches `(on-pick index id)` with the selected marker's id (Shift:
//! the next marker's), so a host that selects the marker on its phase-0 move
//! gets "click a ring to select it, click a snare to load it there". Markers
//! with `:near` (a member index) split the preview strip into side-by-side
//! strips, one per marker, labelled and drawn in the marker's colour. Each
//! strip is a number picker over the members: a vertical drag (up = next,
//! one member per `STRIP_STEP_PX`) or the scroll wheel dispatches
//! `(on-pick index id)` for that strip's marker at every step.

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
/// Latent mode: a press on a dot that travels less than this many design
/// pixels before release is a pick, not a drag.
const CLICK_SLOP_PX: f32 = 3.0;
/// Latent mode: the score range `on-move` reports, the DSP's z clip.
const SCORE_LIMIT: f32 = 3.0;
/// Markers mode: design pixels of vertical drag per member step on a strip.
const STRIP_STEP_PX: f32 = 12.0;

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

/// The last few modal previews (markers mode draws one per marker each frame,
/// so a single entry would recompute every strip on every frame).
#[derive(Default)]
struct PreviewCache {
    entries: Vec<((usize, [u32; 16]), Rc<Vec<f32>>)>,
}

const PREVIEW_CACHE_ENTRIES: usize = 4;

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
        if let Some((_, samples)) = cache.entries.iter().find(|(k, _)| *k == key) {
            return samples.clone();
        }
        let samples = Rc::new(preview_samples(kind, slots, morph, voice));
        if cache.entries.len() >= PREVIEW_CACHE_ENTRIES {
            cache.entries.remove(0);
        }
        cache.entries.push((key, samples.clone()));
        samples
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

    /// The inverse of `point` (before its edge clamp): the score under a
    /// pointer, clamped to `-SCORE_LIMIT..SCORE_LIMIT` on each axis.
    fn score(&self, col: f32, row: f32) -> [f32; 2] {
        [
            ((col - self.center[0]) / self.sd[0].max(1e-6)).clamp(-SCORE_LIMIT, SCORE_LIMIT),
            ((self.center[1] - row) / self.sd[1].max(1e-6)).clamp(-SCORE_LIMIT, SCORE_LIMIT),
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

/// A custom payload tagged with its kind: `(:pick index slot)` or
/// `(:move x y phase)`.
fn tagged_event(kind: &str, numbers: &[f64]) -> WidgetEvent {
    let mut items = vec![Rc::new(RefCell::new(Value::Keyword(kind.to_string())))];
    items.extend(numbers.iter().map(|n| Rc::new(RefCell::new(Value::Number(*n)))));
    WidgetEvent::Custom(Value::List(items))
}

fn pick_event(index: usize, slot: usize) -> WidgetEvent {
    tagged_event("pick", &[index as f64, slot as f64])
}

fn move_event(score: [f32; 2], phase: u8) -> WidgetEvent {
    tagged_event("move", &[score[0] as f64, score[1] as f64, phase as f64])
}

/// A move: `(x y phase)`, or `(x y phase id)` when it moves a marker, at the
/// pointer plus the grab offset, clamped to the z range.
fn gesture_move(score: [f32; 2], phase: u8, marker: Option<(i64, [f32; 2])>) -> WidgetEvent {
    match marker {
        None => move_event(score, phase),
        Some((id, offset)) => {
            let at = |k: usize| (score[k] + offset[k]).clamp(-SCORE_LIMIT, SCORE_LIMIT) as f64;
            tagged_event("move", &[at(0), at(1), phase as f64, id as f64])
        }
    }
}

/// A prop that is present and not nil / false.
fn prop_set(props: &HashMap<String, Value>, key: &str) -> bool {
    props.get(key).is_some_and(|value| !matches!(value, Value::Nil | Value::Bool(false)))
}

/// A boolean prop as a host binding delivers it (bool or 0/1).
fn prop_on(props: &HashMap<String, Value>, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(on)) => *on,
        Some(Value::Number(n)) => *n != 0.0,
        _ => false,
    }
}

/// Bottom right of the plot while `:recording` and `:playing` are both on:
/// a drag then prints p-locks instead of moving the base values.
fn recording_badge(
    props: &HashMap<String, Value>,
    plot_rect: Rect,
    background: Color,
    cell_w: f32,
    cell_h: f32,
    viewport: WidgetViewport,
) -> Vec<GpuPrimitive> {
    if !(prop_on(props, "recording") && prop_on(props, "playing")) {
        return Vec::new();
    }
    let red = resolve_named_color(props, "recording-color", Color::rgba(0.93, 0.27, 0.24, 1.0));
    let label = "recording";
    let (pad_w, pad_h) = (super::ui_design_px(6.0) / cell_w, super::ui_design_px(4.0) / cell_h);
    let dot_w = super::ui_design_px(10.0) / cell_w;
    let text_w = label.chars().count() as f32 * 0.55;
    let width = pad_w * 2.0 + dot_w + text_w;
    let height = 1.0 + pad_h;
    let badge = Rect {
        col: plot_rect.col + plot_rect.width - width - pad_w,
        row: plot_rect.row + plot_rect.height - height - pad_h,
        width,
        height,
    };
    let fill = mixed(background, red, 0.2);
    let mut prims = vec![
        GpuPrimitive::Rect(GpuRectPrimitive { rect: badge, color: fill }),
    ];
    let mut mesh = ShadedMesh::new();
    mesh.push_disc([badge.col + pad_w + dot_w * 0.4, badge.row + badge.height * 0.5], 2.6, red, viewport);
    mesh.push_into(&mut prims);
    let mut primitive = text(badge.row + pad_h * 0.5, badge.col + pad_w + dot_w, label.to_string(), 8.5, mixed(fill, red, 1.0));
    if let GpuPrimitive::ProportionalText(t) = &mut primitive {
        t.align_width = text_w;
    }
    prims.push(primitive);
    prims
}

/// Latent mode is on when the host wants continuous moves.
fn latent_mode(props: &HashMap<String, Value>) -> bool {
    prop_set(props, "on-move")
}

/// The point `:x` / `:y` / `:size` place directly, when `:x` is given.
fn direct_point(props: &HashMap<String, Value>) -> Option<[f32; 3]> {
    prop_set(props, "x").then(|| {
        [
            get_f32_prop(props, "x", 0.0).clamp(-SCORE_LIMIT, SCORE_LIMIT),
            get_f32_prop(props, "y", 0.0).clamp(-SCORE_LIMIT, SCORE_LIMIT),
            get_f32_prop(props, "size", 0.0).clamp(-SCORE_LIMIT, SCORE_LIMIT),
        ]
    })
}

/// One `:markers` entry: a point in score units with its own label, colour
/// and (optionally) the member the preview strip shows for it.
#[derive(Clone, Debug)]
struct Marker {
    id: i64,
    x: f32,
    y: f32,
    size: f32,
    label: String,
    near: Option<usize>,
    color: Option<Color>,
}

fn map_number(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<f32> {
    match &*map.get(key)?.borrow() {
        Value::Number(n) if n.is_finite() => Some(*n as f32),
        _ => None,
    }
}

/// The `:markers` list; empty when unset (single-point / VILLAIN modes).
/// A marker without `:id` takes its list position.
fn markers_from_props(props: &HashMap<String, Value>, count: usize) -> Vec<Marker> {
    let Some(Value::List(items)) = props.get("markers") else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(position, item)| {
            let Value::Map(map) = &*item.borrow() else {
                return None;
            };
            let coord = |key| map_number(map, key).unwrap_or(0.0).clamp(-SCORE_LIMIT, SCORE_LIMIT);
            let label = match map.get("label").map(|cell| cell.borrow().clone()) {
                Some(Value::String(label)) => label,
                _ => String::new(),
            };
            let near = map_number(map, "near")
                .filter(|_| count > 0)
                .map(|n| ((n.max(0.0) + 0.5).floor() as usize).min(count - 1));
            let color = map.get("color").and_then(|cell| theme::parse_color_value(&cell.borrow()));
            Some(Marker {
                id: map_number(map, "id").map_or(position as i64, |id| id.round() as i64),
                x: coord("x"),
                y: coord("y"),
                size: coord("size"),
                label,
                near,
                color,
            })
        })
        .collect()
}

/// Position in `markers` of the `:selected` id (the first marker by default).
fn selected_marker(props: &HashMap<String, Value>, markers: &[Marker]) -> usize {
    if !prop_set(props, "selected") {
        return 0;
    }
    let id = get_f32_prop(props, "selected", 0.0).round() as i64;
    markers.iter().position(|marker| marker.id == id).unwrap_or(0)
}

fn marker_ring_px(size: f32) -> f32 {
    5.0 + (size.clamp(-3.0, 3.0) + 3.0) * 1.2
}

/// The marker whose ring is under the pointer, the selected one first (it is
/// drawn on top).
fn marker_at(
    markers: &[Marker],
    selected: usize,
    plot: &Plot,
    col: f32,
    row: f32,
    cell_w: f32,
    cell_h: f32,
) -> Option<usize> {
    let order = std::iter::once(selected).chain((0..markers.len()).filter(|&i| i != selected));
    let mut best: Option<(usize, f32)> = None;
    for i in order {
        let marker = &markers[i];
        let p = plot.point(marker.x, marker.y);
        let distance = ((p[0] - col) * cell_w).hypot((p[1] - row) * cell_h);
        let limit = super::ui_design_px(marker_ring_px(marker.size) + 4.0).max(super::ui_design_px(PICK_RADIUS_PX));
        if distance <= limit && best.is_none_or(|(_, d)| distance < d - 1e-3) {
            best = Some((i, distance));
        }
    }
    best.map(|(i, _)| i)
}

/// The marker whose preview strip is under `col`: strips sit side by side in
/// `strip`, one per marker with `:near`.
fn strip_marker_at(markers: &[Marker], strip: Rect, col: f32) -> Option<usize> {
    let previewed: Vec<usize> = (0..markers.len()).filter(|&i| markers[i].near.is_some()).collect();
    if previewed.is_empty() || strip.width <= 0.0 {
        return None;
    }
    let k = ((col - strip.col) / (strip.width / previewed.len() as f32)).floor();
    previewed.get(k.clamp(0.0, (previewed.len() - 1) as f32) as usize).copied()
}

/// A number-picker drag on a marker's preview strip: the marker id, the row
/// the drag began on, the member it began at and the last one dispatched.
#[derive(Clone, Copy, Debug)]
struct StripPress {
    widget_id: u64,
    id: i64,
    row: f32,
    start: usize,
    last: usize,
}

thread_local! {
    static STRIP_PRESS: RefCell<Option<StripPress>> = const { RefCell::new(None) };
}

/// The latent-mode press in flight (one pointer, so one slot): which widget,
/// where it landed, the dot it landed on with the slot a pick would load
/// (Shift read at press time), and whether it has since travelled past
/// `CLICK_SLOP_PX` (after which release is a move, never a pick).
#[derive(Clone, Copy, Debug)]
struct LatentPress {
    widget_id: u64,
    at: (f32, f32),
    dot: Option<(usize, usize)>,
    travelled: bool,
    /// Markers mode: the marker this gesture moves and the grab offset
    /// (marker minus pointer, in score units) that keeps it from jumping.
    marker: Option<(i64, [f32; 2])>,
}

thread_local! {
    static LATENT_PRESS: RefCell<Option<LatentPress>> = const { RefCell::new(None) };
}

fn travel_px(from: (f32, f32), col: f32, row: f32, cell_w: f32, cell_h: f32) -> f32 {
    ((col - from.0) * cell_w).hypot((row - from.1) * cell_h)
}

fn armed_slot(props: &HashMap<String, Value>, modifiers: KeyModifiers) -> usize {
    let armed = usize::from(get_f32_prop(props, "armed", 0.0) > 0.5);
    if modifiers.contains(KeyModifiers::SHIFT) { 1 - armed } else { armed }
}

/// Latent-mode pointer handling; see the module doc.
#[allow(clippy::too_many_arguments)]
fn latent_mouse_event(
    node: &LayoutNode,
    family: &Family,
    mouse_kind: MouseEventKind,
    col: f32,
    row: f32,
    modifiers: KeyModifiers,
    cell_w: f32,
    cell_h: f32,
) -> MouseEventOutcome {
    let plot = Plot::new(split_rect(node.rect, cell_h).0, family);
    let slop = super::ui_design_px(CLICK_SLOP_PX);
    let pointer = plot.score(col, row);
    match mouse_kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !plot.rect.contains(col, row) {
                // The preview strip: a marker's strip starts a picker drag,
                // anything else ignores the whole gesture.
                LATENT_PRESS.with(|press| *press.borrow_mut() = None);
                let markers = markers_from_props(&node.props, family.members.len());
                let press = strip_marker_at(&markers, split_rect(node.rect, cell_h).1, col).map(|i| {
                    let near = markers[i].near.unwrap_or(0);
                    StripPress { widget_id: node.widget_id, id: markers[i].id, row, start: near, last: near }
                });
                STRIP_PRESS.with(|slot| *slot.borrow_mut() = press);
                return MouseEventOutcome::Consume;
            }
            let markers = markers_from_props(&node.props, family.members.len());
            let (dot, marker) = if markers.is_empty() {
                let dot = nearest_member(family, &plot, col, row, cell_w, cell_h)
                    .map(|index| (index, armed_slot(&node.props, modifiers)));
                (dot, None)
            } else {
                let selected = selected_marker(&node.props, &markers);
                match marker_at(&markers, selected, &plot, col, row, cell_w, cell_h) {
                    // Grab the ring where it is: phase 0 at its own position.
                    Some(i) => {
                        let m = &markers[i];
                        (None, Some((m.id, [m.x - pointer[0], m.y - pointer[1]])))
                    }
                    None => {
                        let target = if modifiers.contains(KeyModifiers::SHIFT) {
                            (selected + 1) % markers.len()
                        } else {
                            selected
                        };
                        let dot = nearest_member(family, &plot, col, row, cell_w, cell_h)
                            .map(|index| (index, markers[target].id.max(0) as usize));
                        (dot, Some((markers[selected].id, [0.0, 0.0])))
                    }
                }
            };
            LATENT_PRESS.with(|press| {
                *press.borrow_mut() =
                    Some(LatentPress { widget_id: node.widget_id, at: (col, row), dot, travelled: false, marker });
            });
            match dot {
                Some(_) => MouseEventOutcome::Consume,
                None => MouseEventOutcome::Dispatch(gesture_move(pointer, 0, marker)),
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let strip_step = STRIP_PRESS.with(|slot| {
                let mut slot = slot.borrow_mut();
                let state = slot.as_mut().filter(|state| state.widget_id == node.widget_id)?;
                let steps = ((state.row - row) * cell_h / super::ui_design_px(STRIP_STEP_PX)).round() as i64;
                let last = family.members.len().saturating_sub(1) as i64;
                let index = (state.start as i64 + steps).clamp(0, last) as usize;
                let changed = index != state.last;
                state.last = index;
                Some(changed.then_some((index, state.id)))
            });
            if let Some(step) = strip_step {
                return match step {
                    Some((index, id)) => MouseEventOutcome::Dispatch(pick_event(index, id.max(0) as usize)),
                    None => MouseEventOutcome::Consume,
                };
            }
            // A drag that leaves a dot opens the gesture there (phase 0).
            let gesture = LATENT_PRESS.with(|press| {
                let mut press = press.borrow_mut();
                let state = press.as_mut().filter(|state| state.widget_id == node.widget_id)?;
                if state.dot.is_some()
                    && !state.travelled
                    && travel_px(state.at, col, row, cell_w, cell_h) < slop
                {
                    return None;
                }
                let phase = if state.dot.is_some() && !state.travelled { 0 } else { 1 };
                state.travelled = true;
                Some((phase, state.marker))
            });
            match gesture {
                Some((phase, marker)) => MouseEventOutcome::Dispatch(gesture_move(pointer, phase, marker)),
                None => MouseEventOutcome::Consume,
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if STRIP_PRESS.with(|slot| slot.borrow_mut().take()).is_some_and(|s| s.widget_id == node.widget_id) {
                return MouseEventOutcome::Consume;
            }
            let Some(state) = LATENT_PRESS
                .with(|press| press.borrow_mut().take())
                .filter(|state| state.widget_id == node.widget_id)
            else {
                return MouseEventOutcome::Consume;
            };
            match state.dot {
                Some((index, slot))
                    if !state.travelled && travel_px(state.at, col, row, cell_w, cell_h) < slop =>
                {
                    MouseEventOutcome::Dispatch(pick_event(index, slot))
                }
                _ => MouseEventOutcome::Dispatch(gesture_move(pointer, 2, state.marker)),
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if !plot.rect.contains(col, row) => {
            let markers = markers_from_props(&node.props, family.members.len());
            let Some(i) = strip_marker_at(&markers, split_rect(node.rect, cell_h).1, col) else {
                return MouseEventOutcome::Ignore;
            };
            let near = markers[i].near.unwrap_or(0);
            let index = match mouse_kind {
                MouseEventKind::ScrollUp => (near + 1).min(family.members.len().saturating_sub(1)),
                _ => near.saturating_sub(1),
            };
            if index == near {
                return MouseEventOutcome::Consume;
            }
            MouseEventOutcome::Dispatch(pick_event(index, markers[i].id.max(0) as usize))
        }
        _ => MouseEventOutcome::Ignore,
    }
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
        face: crate::ui::glyph_atlas::TextFace::Regular,
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

/// Where each marker's ring label goes, `(marker, (row, col, width))`, in
/// marker order: right of the ring, or left of it near the plot's right
/// edge; a label overlapping one already placed tries the other side, then
/// the line below.
fn label_slots(
    markers: &[Marker],
    centers: &[[f32; 2]],
    plot_rect: Rect,
    pad_w: f32,
    cell_w: f32,
    placed: &mut Vec<(f32, f32, f32)>,
) -> Vec<(usize, (f32, f32, f32))> {
    let overlaps = |placed: &[(f32, f32, f32)], row: f32, col: f32, width: f32| {
        placed.iter().any(|&(r, c, w)| (r - row).abs() < 0.9 && col < c + w && c < col + width)
    };
    let mut out = Vec::new();
    for (i, marker) in markers.iter().enumerate() {
        if marker.label.is_empty() {
            continue;
        }
        let gap = super::ui_design_px(marker_ring_px(marker.size) + 4.0) / cell_w;
        let width = marker.label.chars().count() as f32 * 0.6;
        let p = centers[i];
        let right = p[0] + gap;
        let left = p[0] - gap - width;
        let fits_right = right + width <= plot_rect.col + plot_rect.width - pad_w * 0.5;
        let sides = if fits_right { [right, left] } else { [left, right] };
        let row = p[1] - 0.5;
        let slot = sides
            .iter()
            .map(|&col| (row, col))
            .chain(std::iter::once((row + 1.0, sides[0])))
            .find(|&(row, col)| !overlaps(placed, row, col, width))
            .unwrap_or((row + 1.0, sides[0]));
        placed.push((slot.0, slot.1, width));
        out.push((i, (slot.0, slot.1, width)));
    }
    out
}

/// One preview trace drawn into `inset` (a strip, or one side of a split
/// strip), in `color`.
#[allow(clippy::too_many_arguments)]
fn push_trace(
    prims: &mut Vec<GpuPrimitive>,
    mesh: &mut ShadedMesh,
    trace: PreviewTrace,
    inset: Rect,
    color: Color,
    pad_w: f32,
    hair_h: f32,
    viewport: WidgetViewport,
) {
    let cell_w = viewport.cell_w.max(1.0);
    let x0 = inset.col + pad_w;
    let span = inset.width - pad_w * 2.0;
    if span <= 0.0 {
        return;
    }
    let columns = ((span * cell_w / super::ui_design_px(1.0)) as usize).clamp(16, 600);
    let mid = inset.row + inset.height * 0.5;
    let half = inset.height * 0.40;
    let column_x = |c: usize| x0 + span * c as f32 / (columns - 1).max(1) as f32;
    match trace {
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
            mesh.push_polyline(&points, with_alpha(color, 0.85), viewport, 0.45);
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
            let fill = with_alpha(color, 0.22);
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
            mesh.push_polyline(&top, with_alpha(color, 0.85), viewport, 0.45);
            mesh.push_polyline(&bottom, with_alpha(color, 0.85), viewport, 0.45);
        }
        _ => {}
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
            "x",
            "y",
            "size",
            "markers",
            "selected",
            "recording",
            "playing",
        ]
    }

    fn doc(&self) -> &'static str {
        "Specific to the VILLAIN drum instruments (Kick, Snare, Hat): plots the \
instrument's family of fitted hits as dots on its two main axes (dot size = third axis), \
highlights members A and B, shows the blend point on the A-B line and where \
Exaggerate and the axis offsets push it, and previews the morph in a strip below. \
:file is the instrument's family.json sidecar (nothing draws without it). Bind the \
instrument's params: :a and :b (member indices), :blend 0..1, :exaggerate, :pc1 :pc2 \
:pc3 (axis offsets) and the voice params (:spread, :beat, :tilt, :drop-time, :decay, \
:bend, :body, :ring, :tune) for the preview. Clicking a dot calls :on-pick with \
(index slot): slot is :armed (0 = A, 1 = B), flipped when Shift is held. :x-label and \
:y-label name the axes; :accent colours A and the morph marker. \
Latent mode: an :on-move callback (lambda (x y phase)) makes the plot a continuous 2D \
control in score units (-3..3); phase is 0 on press, 1 while dragging, 2 on release, so \
the host can group one undo per drag. A short press and release on a dot still picks it. \
:x :y :size then place the ring directly (lead from member A). \
Several points: :markers (list (dict :id 0 :x .. :y .. :size .. :label \"START\" :near i)), \
:selected id; a press on a ring grabs that marker, elsewhere moves the selected one, \
on-move gets (x y phase id), a dot click calls (on-pick index id) for the selected marker \
(Shift: the next), and markers with :near get side-by-side preview strips; dragging a strip \
vertically or scrolling it steps (on-pick index id) through the members. \
:recording and :playing (bind #'transport.recording and #'transport.playing) show a \
\"recording\" badge in the plot's bottom right while both are on: drags then print p-locks. \
Example: (family-map :file (vk-family-file) :a a :b b :blend blend :armed 0 :x-label \"deep\" \
:y-label \"swell\" :on-pick (lambda (index slot) (set-member! slot index)))"
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
            "on-move",
            "x",
            "y",
            "size",
            "markers",
            "selected",
            "recording",
            "playing",
            "recording-color",
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
        let local_row = super::scroll::event_content_row(local_row);
        if latent_mode(&node.props) {
            if !matches!(
                mouse_kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
                    | MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
            ) {
                return MouseEventOutcome::Ignore;
            }
            let Some(family) = family_from_props(&node.props) else {
                return MouseEventOutcome::Ignore;
            };
            return latent_mouse_event(
                node,
                &family,
                mouse_kind,
                local_col,
                local_row,
                modifiers,
                cell_w.max(1.0),
                cell_h.max(1.0),
            );
        }
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
            Some(index) => MouseEventOutcome::Dispatch(pick_event(index, armed_slot(&node.props, modifiers))),
            None => MouseEventOutcome::Consume,
        }
    }

    /// Drags follow the pointer outside the rect (latent mode). The trait
    /// cannot see the node, so a family-map without `:on-move` captures its
    /// drags too; `mouse_event` then ignores them and consumes the release,
    /// so nothing is dispatched that was not before.
    fn captures_drag(&self) -> bool {
        true
    }

    /// The raw pointer arrives during a drag; `Plot::score` clamps it.
    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::List(items)) = event else {
            return None;
        };
        let (kind, rest) = items.split_first()?;
        let key = match &*kind.borrow() {
            Value::Keyword(kind) if kind == "move" => "on-move",
            Value::Keyword(kind) if kind == "pick" => "on-pick",
            _ => return None,
        };
        let callback = node.props.get(key)?.clone();
        let args = rest
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
        let point = direct_point(props);
        let mut morph = morph_from_props(props, family.members.len());
        if point.is_some() {
            // A point, not a morph: A alone, as fitted, is the reference.
            morph.b = morph.a;
            morph.blend = 0.0;
        }
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
        let markers = markers_from_props(props, family.members.len());
        if !markers.is_empty() {
            let selected = selected_marker(props, &markers);
            let colors: Vec<Color> = markers
                .iter()
                .enumerate()
                .map(|(i, marker)| marker.color.unwrap_or(if i == 0 { accent } else { b_color }))
                .collect();
            let centers: Vec<[f32; 2]> = markers.iter().map(|m| plot.point(m.x, m.y)).collect();
            // The morph path, START -> END, faint.
            if centers.len() > 1 {
                mesh.push_polyline(&centers, with_alpha(line_color, 0.45), viewport, 0.6);
            }
            // Members, the markers' nearest ones on top in their colours.
            let near_color = |i: usize| {
                markers.iter().zip(&colors).rev().find(|(m, _)| m.near == Some(i)).map(|(_, c)| *c)
            };
            for (_, score) in family.scores.iter().enumerate().filter(|(i, _)| near_color(*i).is_none()) {
                mesh.push_disc(plot.point(score[0], score[1]), dot_radius_px(score[2]), dot_color, viewport);
            }
            for (i, score) in family.scores.iter().enumerate() {
                if let Some(color) = near_color(i) {
                    mesh.push_disc(plot.point(score[0], score[1]), dot_radius_px(score[2]), color, viewport);
                }
            }
            // Leads from each marker's nearest member, then the rings, the
            // selected one last (on top), heavier, with a soft halo.
            let order: Vec<usize> = (0..markers.len()).filter(|&i| i != selected).chain([selected]).collect();
            for &i in &order {
                if let Some(near) = markers[i].near {
                    let score = family.scores[near];
                    dashed(&mut mesh, plot.point(score[0], score[1]), centers[i], with_alpha(colors[i], 0.7), viewport);
                }
            }
            for &i in &order {
                let radius = marker_ring_px(markers[i].size);
                if i == selected {
                    mesh.push_disc(centers[i], radius, with_alpha(colors[i], 0.16), viewport);
                    mesh.push_polyline(&ring_points(centers[i], radius, viewport), colors[i], viewport, 1.15);
                    mesh.push_disc(centers[i], 1.8, colors[i], viewport);
                } else {
                    mesh.push_polyline(&ring_points(centers[i], radius, viewport), colors[i], viewport, 0.7);
                }
            }

            // Strip: one side-by-side preview per marker that names a member.
            let previewed: Vec<usize> = (0..markers.len()).filter(|&i| markers[i].near.is_some()).collect();
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive { rect: strip, color: inset_color }));
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect { height: hair_h, ..strip },
                color: with_alpha(grid_color, 2.2),
            }));
            let mut labels = Vec::new();
            let voice = voice_from_props(props);
            let share = strip.width / previewed.len().max(1) as f32;
            for (k, &i) in previewed.iter().enumerate() {
                let near = markers[i].near.unwrap_or(0);
                let side = Rect { col: strip.col + share * k as f32, width: share, ..strip };
                if k > 0 {
                    prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: Rect { col: side.col - hair_w * 0.5, width: hair_w, ..side },
                        color: with_alpha(grid_color, 2.2),
                    }));
                }
                let morph = Morph { a: near, b: near, blend: 0.0, exaggerate: 0.0, axes: [0.0; 3] };
                let label_h = super::ui_design_px(11.0) / cell_h;
                let trace_rect = Rect { row: side.row + label_h, height: (side.height - label_h).max(hair_h), ..side };
                push_trace(&mut prims, &mut mesh, preview_trace(&family, &morph, &voice), trace_rect, colors[i], pad_w, hair_h, viewport);
                let name = family.members.get(near).cloned().unwrap_or_default();
                let caption = if markers[i].label.is_empty() { name } else { format!("{}  {}", markers[i].label, name) };
                // Top right: the hit's tail is quiet there, its attack is not.
                let width = caption.chars().count() as f32 * 0.6;
                let mut primitive = text(side.row + pad_h * 0.25, side.col + side.width - pad_w - width, caption, 8.5, mixed(inset_color, colors[i], 1.0));
                if let GpuPrimitive::ProportionalText(t) = &mut primitive {
                    t.align_width = width;
                    t.h_align = 1.0;
                }
                labels.push(primitive);
            }
            mesh.push_into(&mut prims);

            // Ring labels, right of the ring (left of it near the edge). A
            // label that would cover one already placed (two rings close
            // together) takes the other side, else moves down a line.
            let mut placed: Vec<(f32, f32, f32)> = Vec::new();
            for (i, (row, col, width)) in label_slots(&markers, &centers, plot_rect, pad_w, cell_w, &mut placed) {
                let mut primitive = text(row, col, markers[i].label.clone(), 9.0, colors[i]);
                if let GpuPrimitive::ProportionalText(t) = &mut primitive {
                    t.align_width = width;
                }
                prims.push(primitive);
            }
            prims.extend(labels);
            prims.extend(recording_badge(props, plot_rect, background, cell_w, cell_h, viewport));
            return prims;
        }
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
        let (ring_center, ring_axis3) = match point {
            Some(point) => {
                let p_point = plot.point(point[0], point[1]);
                dashed(&mut mesh, pa, p_point, accent, viewport);
                (p_point, point[2])
            }
            None => {
                let (blend, moved) = morph_points(&family, &morph);
                let p_blend = plot.point(blend[0], blend[1]);
                let p_moved = plot.point(moved[0], moved[1]);
                dashed(&mut mesh, p_blend, p_moved, accent, viewport);
                mesh.push_disc(p_blend, 3.0, b_color, viewport);
                (p_moved, moved[2])
            }
        };
        let ring = 5.0 + (ring_axis3.clamp(-3.0, 3.0) + 3.0) * 1.2;
        mesh.push_polyline(&ring_points(ring_center, ring, viewport), accent, viewport, 0.75);

        // Morph preview strip under the plot, set off by a hairline.
        let inset = strip;
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: inset,
            color: inset_color,
        }));
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect { height: hair_h, ..inset },
            color: with_alpha(grid_color, 2.2),
        }));
        push_trace(
            &mut prims,
            &mut mesh,
            preview_trace(&family, &morph, &voice_from_props(props)),
            inset,
            b_color,
            pad_w,
            hair_h,
            viewport,
        );
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
        // A point forces a == b; in latent mode the tag is the name alone.
        if point.is_some() || (morph.a == morph.b && latent_mode(props)) {
            prims.push(tag(pa, family.members[morph.a].clone(), accent));
        } else if morph.a == morph.b {
            prims.push(tag(pa, format!("ab {}", family.members[morph.a]), accent));
        } else {
            prims.push(tag(pb, format!("b {}", family.members[morph.b]), b_color));
            prims.push(tag(pa, format!("a {}", family.members[morph.a]), accent));
        }
        prims.extend(recording_badge(props, plot_rect, background, cell_w, cell_h, viewport));
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

    /// A 40x20 family-map node over `DOC`, written to a per-test sidecar.
    fn test_node(name: &str, extra: &[(&str, Value)]) -> (LayoutNode, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("family-map-{name}-{}.json", std::process::id()));
        std::fs::write(&path, DOC).unwrap();
        let mut props = HashMap::new();
        props.insert("file".to_string(), Value::String(path.to_string_lossy().into_owned()));
        props.insert("on-pick".to_string(), Value::Keyword("cb".to_string()));
        for (key, value) in extra {
            props.insert(key.to_string(), value.clone());
        }
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
        (node, path)
    }

    /// Mouse event at (col, row), then `handle_event`: the callback and its
    /// numeric args, or None when nothing was dispatched.
    fn send(node: &LayoutNode, kind: MouseEventKind, col: f32, row: f32) -> Option<(Value, Vec<f64>)> {
        match FAMILY_MAP_WIDGET.mouse_event(node, kind, col, row, None, None, KeyModifiers::NONE, 8.0, 16.0) {
            MouseEventOutcome::Dispatch(event) => {
                let output = FAMILY_MAP_WIDGET.handle_event(node, event).expect("routed to a callback");
                let args = output.args.iter().map(|v| match v { Value::Number(n) => *n, _ => f64::NAN }).collect();
                Some((output.callback, args))
            }
            _ => None,
        }
    }

    fn assert_close(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-4, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn family_map_score_inverts_point_inside_the_clamp_margin() {
        let family = parse_family(DOC).unwrap();
        let plot = Plot::new(Rect { col: 2.0, row: 1.0, width: 40.0, height: 15.0 }, &family);
        for (x, y) in [(0.0, 0.0), (1.5, -0.7), (-2.0, 1.8), (0.3, -1.8)] {
            let p = plot.point(x, y);
            let s = plot.score(p[0], p[1]);
            assert!((s[0] - x).abs() < 1e-4 && (s[1] - y).abs() < 1e-4, "({x}, {y}) -> {s:?}");
        }
        let far = plot.score(1e6, -1e6);
        assert_eq!(far, [SCORE_LIMIT, SCORE_LIMIT], "right and above: clamped to the z range");
    }

    #[test]
    fn family_map_latent_drag_moves_with_begin_update_end_phases() {
        let (node, path) = test_node("latent-drag", &[("on-move", Value::Keyword("mv".to_string()))]);
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let start = plot.point(0.5, 0.5);
        let mid = plot.point(-1.0, 1.25);
        let left = MouseButton::Left;
        let (callback, args) = send(&node, MouseEventKind::Down(left), start[0], start[1]).expect("press moves");
        assert_eq!(callback, Value::Keyword("mv".to_string()), "routed to on-move");
        assert_close(&args, &[0.5, 0.5, 0.0]);
        let (_, args) = send(&node, MouseEventKind::Drag(left), mid[0], mid[1]).expect("drag moves");
        assert_close(&args, &[-1.0, 1.25, 1.0]);
        // Far outside the rect: still tracked, clamped to the z range.
        let (_, args) = send(&node, MouseEventKind::Drag(left), 1e4, -1e4).expect("captured drag");
        assert_close(&args, &[3.0, 3.0, 1.0]);
        let (_, args) = send(&node, MouseEventKind::Up(left), mid[0], mid[1]).expect("release ends");
        assert_close(&args, &[-1.0, 1.25, 2.0]);
        assert!(send(&node, MouseEventKind::Drag(left), mid[0], mid[1]).is_none(), "no gesture after release");
        // A press on the preview strip is ignored, drag and release included.
        let strip = split_rect(node.rect, 16.0).1;
        let (col, row) = (strip.col + 5.0, strip.row + strip.height * 0.5);
        assert!(send(&node, MouseEventKind::Down(left), col, row).is_none());
        assert!(send(&node, MouseEventKind::Drag(left), start[0], start[1]).is_none());
        assert!(send(&node, MouseEventKind::Up(left), start[0], start[1]).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_latent_press_release_on_a_dot_picks_but_a_drag_off_it_moves() {
        let (node, path) = test_node("latent-pick", &[
            ("on-move", Value::Keyword("mv".to_string())),
            ("armed", Value::Number(0.0)),
        ]);
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let dot = plot.point(family.scores[1][0], family.scores[1][1]);
        let left = MouseButton::Left;
        assert!(send(&node, MouseEventKind::Down(left), dot[0], dot[1]).is_none(), "a dot press waits");
        // Jitter inside the click slop (1 px) neither moves nor cancels the pick.
        assert!(send(&node, MouseEventKind::Drag(left), dot[0] + 0.125, dot[1]).is_none());
        let (callback, args) = send(&node, MouseEventKind::Up(left), dot[0] + 0.125, dot[1]).expect("picks");
        assert_eq!(callback, Value::Keyword("cb".to_string()), "routed to on-pick");
        assert_close(&args, &[1.0, 0.0]);

        // Dragging off the dot drags the point; coming back and releasing on
        // the dot does not turn the drag into a pick.
        assert!(send(&node, MouseEventKind::Down(left), dot[0], dot[1]).is_none());
        let away = plot.point(0.0, 0.0);
        let (_, args) = send(&node, MouseEventKind::Drag(left), away[0], away[1]).expect("drag off the dot moves");
        assert_close(&args, &[0.0, 0.0, 0.0]);
        let (_, args) = send(&node, MouseEventKind::Drag(left), away[0] + 1.0, away[1]).expect("and keeps moving");
        assert_eq!(args[2], 1.0, "the gesture opened on leaving the dot");
        let (callback, args) = send(&node, MouseEventKind::Up(left), dot[0], dot[1]).expect("release ends the drag");
        assert_eq!(callback, Value::Keyword("mv".to_string()));
        assert_eq!(args[2], 2.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_without_on_move_ignores_drags() {
        let (node, path) = test_node("no-latent", &[]);
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let empty = plot.point(0.0, 0.0);
        let left = MouseButton::Left;
        let outcome = |kind| FAMILY_MAP_WIDGET.mouse_event(&node, kind, empty[0], empty[1], Some((0.0, 0.0)), None, KeyModifiers::NONE, 8.0, 16.0);
        assert!(matches!(outcome(MouseEventKind::Down(left)), MouseEventOutcome::Consume));
        assert!(matches!(outcome(MouseEventKind::Drag(left)), MouseEventOutcome::Ignore));
        assert!(matches!(outcome(MouseEventKind::Up(left)), MouseEventOutcome::Consume));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn clicking_a_dot_picks_it_into_the_armed_slot() {
        let (node, path) = test_node("pick", &[("armed", Value::Number(1.0))]);
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

    fn cell(value: Value) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(value))
    }

    /// `(list (dict :id 0 :x .. :y .. :label "START" :near 0) (dict :id 1 ... "END" :near 1))`.
    fn two_markers(start: [f32; 2], end: [f32; 2]) -> Value {
        let marker = |id: f64, at: [f32; 2], label: &str, near: f64| {
            cell(Value::Map(HashMap::from([
                ("id".to_string(), cell(Value::Number(id))),
                ("x".to_string(), cell(Value::Number(at[0] as f64))),
                ("y".to_string(), cell(Value::Number(at[1] as f64))),
                ("size".to_string(), cell(Value::Number(0.0))),
                ("label".to_string(), cell(Value::String(label.to_string()))),
                ("near".to_string(), cell(Value::Number(near))),
            ])))
        };
        Value::List(vec![marker(0.0, start, "START", 0.0), marker(1.0, end, "END", 1.0)])
    }

    fn marker_node(name: &str, selected: Option<f64>) -> (LayoutNode, std::path::PathBuf) {
        let mut extra = vec![
            ("on-move", Value::Keyword("mv".to_string())),
            ("markers", two_markers([0.0, 1.5], [-1.5, -1.5])),
        ];
        if let Some(selected) = selected {
            extra.push(("selected", Value::Number(selected)));
        }
        test_node(name, &extra)
    }

    #[test]
    fn family_map_markers_parse_with_ids_labels_and_nearest() {
        let (node, path) = marker_node("markers-parse", Some(1.0));
        let markers = markers_from_props(&node.props, 2);
        assert_eq!(markers.len(), 2);
        assert_eq!((markers[0].id, markers[0].label.as_str(), markers[0].near), (0, "START", Some(0)));
        assert_eq!((markers[1].id, markers[1].x, markers[1].y), (1, -1.5, -1.5));
        assert_eq!(selected_marker(&node.props, &markers), 1);
        assert_eq!(selected_marker(&HashMap::new(), &markers), 0, "the first marker by default");
        assert!(markers_from_props(&HashMap::new(), 2).is_empty(), "no :markers, single-point mode");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_markers_press_on_a_ring_grabs_it_without_jumping() {
        let (node, path) = marker_node("markers-grab", Some(0.0));
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let end = plot.point(-1.5, -1.5);
        let left = MouseButton::Left;
        // Press a little off END's centre (inside its ring): END is grabbed
        // where it is, although START is the selected marker.
        let press = [end[0] + 0.5, end[1] + 0.25];
        let (callback, args) = send(&node, MouseEventKind::Down(left), press[0], press[1]).expect("grabs");
        assert_eq!(callback, Value::Keyword("mv".to_string()));
        assert_close(&args, &[-1.5, -1.5, 0.0, 1.0]);
        // Dragging carries the grab offset: the marker moves by the pointer's delta.
        let target = plot.point(0.5, 0.25);
        let (_, args) = send(&node, MouseEventKind::Drag(left), target[0] + 0.5, target[1] + 0.25).expect("drags");
        assert_close(&args, &[0.5, 0.25, 1.0, 1.0]);
        let (_, args) = send(&node, MouseEventKind::Up(left), target[0] + 0.5, target[1] + 0.25).expect("ends");
        assert_close(&args, &[0.5, 0.25, 2.0, 1.0]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_markers_press_elsewhere_moves_the_selected_marker() {
        let (node, path) = marker_node("markers-empty", Some(1.0));
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let empty = plot.point(1.0, 1.0);
        let left = MouseButton::Left;
        let (_, args) = send(&node, MouseEventKind::Down(left), empty[0], empty[1]).expect("moves");
        assert_close(&args, &[1.0, 1.0, 0.0, 1.0]);
        let (_, args) = send(&node, MouseEventKind::Up(left), empty[0], empty[1]).expect("ends");
        assert_close(&args, &[1.0, 1.0, 2.0, 1.0]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_markers_dot_click_loads_into_the_selected_marker() {
        let (node, path) = marker_node("markers-pick", Some(1.0));
        let family = family_from_props(&node.props).unwrap();
        let plot = Plot::new(split_rect(node.rect, 16.0).0, &family);
        let dot = plot.point(family.scores[0][0], family.scores[0][1]);
        let left = MouseButton::Left;
        assert!(send(&node, MouseEventKind::Down(left), dot[0], dot[1]).is_none(), "a dot press waits");
        let (callback, args) = send(&node, MouseEventKind::Up(left), dot[0], dot[1]).expect("picks");
        assert_eq!(callback, Value::Keyword("cb".to_string()));
        assert_close(&args, &[0.0, 1.0]);
        // Shift loads into the next marker instead.
        let shifted = FAMILY_MAP_WIDGET.mouse_event(&node, MouseEventKind::Down(left), dot[0], dot[1], None, None, KeyModifiers::SHIFT, 8.0, 16.0);
        assert!(matches!(shifted, MouseEventOutcome::Consume));
        let (_, args) = send(&node, MouseEventKind::Up(left), dot[0], dot[1]).expect("picks");
        assert_close(&args, &[0.0, 0.0]);
        // A drag that starts on a dot moves the selected marker off it.
        assert!(send(&node, MouseEventKind::Down(left), dot[0], dot[1]).is_none());
        let away = plot.point(0.5, -0.5);
        let (_, args) = send(&node, MouseEventKind::Drag(left), away[0], away[1]).expect("moves");
        assert_close(&args, &[0.5, -0.5, 0.0, 1.0]);
        let _ = send(&node, MouseEventKind::Up(left), away[0], away[1]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_marker_strips_pick_members_like_a_number_picker() {
        let (node, path) = marker_node("markers-strip-picker", Some(0.0));
        let strip = split_rect(node.rect, 16.0).1;
        let (start_col, end_col) = (strip.col + strip.width * 0.25, strip.col + strip.width * 0.75);
        let row = strip.row + strip.height * 0.5;
        let step = super::super::ui_design_px(STRIP_STEP_PX) / 16.0;
        let left = MouseButton::Left;
        // START strip (near 0): a press alone dispatches nothing.
        assert!(send(&node, MouseEventKind::Down(left), start_col, row).is_none());
        assert!(send(&node, MouseEventKind::Drag(left), start_col, row - step * 0.3).is_none(), "under one step");
        let (callback, args) = send(&node, MouseEventKind::Drag(left), start_col, row - step).expect("steps up");
        assert_eq!(callback, Value::Keyword("cb".to_string()));
        assert_close(&args, &[1.0, 0.0]);
        assert!(send(&node, MouseEventKind::Drag(left), start_col, row - step * 5.0).is_none(), "clamped at the last member");
        let (_, args) = send(&node, MouseEventKind::Drag(left), start_col, row + step).expect("steps back down");
        assert_close(&args, &[0.0, 0.0]);
        assert!(send(&node, MouseEventKind::Up(left), start_col, row + step).is_none(), "release dispatches nothing");
        // END strip (near 1): the wheel steps by one, clamped at the ends.
        let (_, args) = send(&node, MouseEventKind::ScrollDown, end_col, row).expect("scrolls down");
        assert_close(&args, &[0.0, 1.0]);
        assert!(send(&node, MouseEventKind::ScrollUp, end_col, row).is_none(), "already the last member");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_markers_draw_side_by_side_strips() {
        let (node, path) = marker_node("markers-draw", None);
        let viewport = WidgetViewport::window(8.0, 16.0, 800.0, 600.0, 0.0);
        let prims = FAMILY_MAP_WIDGET.build_primitives("family-map", &node, viewport);
        let captions: Vec<String> = prims
            .iter()
            .filter_map(|p| match p {
                GpuPrimitive::ProportionalText(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(captions.contains(&"START".to_string()) && captions.contains(&"END".to_string()), "{captions:?}");
        assert!(captions.contains(&"START  51".to_string()) && captions.contains(&"END  52".to_string()), "{captions:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn family_map_shows_the_recording_badge_only_while_recording_and_playing() {
        let viewport = WidgetViewport::window(8.0, 16.0, 800.0, 600.0, 0.0);
        let badge = |name: &str, recording: bool, playing: bool, markers: bool| {
            let (mut node, path) = if markers {
                marker_node(name, None)
            } else {
                test_node(name, &[])
            };
            node.props.insert("recording".to_string(), Value::Bool(recording));
            node.props.insert("playing".to_string(), Value::Bool(playing));
            let prims = FAMILY_MAP_WIDGET.build_primitives("family-map", &node, viewport);
            let _ = std::fs::remove_file(path);
            prims.iter().find_map(|p| match p {
                GpuPrimitive::ProportionalText(t) if t.text == "recording" => Some((t.row, t.col)),
                _ => None,
            })
        };
        assert!(badge("rec-off", false, true, false).is_none(), "playing alone");
        assert!(badge("rec-stopped", true, false, true).is_none(), "recording alone");
        let (plot, _) = split_rect(Rect { col: 0.0, row: 0.0, width: 40.0, height: 20.0 }, 16.0);
        for (name, markers) in [("rec-point", false), ("rec-markers", true)] {
            let (row, col) = badge(name, true, true, markers).expect("badge while recording and playing");
            assert!(col > plot.col + plot.width * 0.5 && row > plot.row + plot.height * 0.5, "bottom right: {row} {col}");
        }
    }

    #[test]
    fn family_map_marker_labels_do_not_overlap_when_rings_coincide() {
        let (node, path) = marker_node("markers-labels", None);
        let markers = {
            let mut m = markers_from_props(&node.props, 2);
            m[1].x = m[0].x;
            m[1].y = m[0].y;
            m
        };
        let centers = vec![[20.0, 5.0], [20.0, 5.0]];
        let rect = Rect { col: 0.0, row: 0.0, width: 40.0, height: 15.0 };
        let slots = label_slots(&markers, &centers, rect, 1.0, 8.0, &mut Vec::new());
        let (a, b) = (slots[0].1, slots[1].1);
        assert!(a.1 > 20.0 && b.1 + b.2 < 20.0, "START right of the ring, END left of it: {slots:?}");
        let _ = std::fs::remove_file(path);
    }
}
