use super::*;

/// Resolve display color without touching the authored/persisted track palette.
pub(crate) fn track_display_color(
    app: &app::App,
    track: usize,
) -> sequencer::track_color::TrackColor {
    let color = app
        .track_colors
        .get(track)
        .copied()
        .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
        .clamped();
    let [r, g, b] = themed_track_rgb([color.r, color.g, color.b]);
    sequencer::track_color::TrackColor::new(r, g, b).clamped()
}

/// Track groups share the track display tint, not a separate authored palette.
pub(crate) fn themed_track_rgb(color: [f32; 3]) -> [f32; 3] {
    let (tint, palette) = eseqlisp::theme::track_display_key();
    tinted_rgb(palette_snapped_rgb(color, &palette), tint)
}

/// Display tint for the p-lock variant and sound palette color set. Applied at
/// publish time only: the variant registry and sound entities keep their
/// palette indices, so switching themes never rewrites project state.
pub(crate) fn themed_variant_rgb(color: [f32; 3]) -> [f32; 3] {
    let (tint, palette) = eseqlisp::theme::variant_display_key();
    tinted_rgb(palette_snapped_rgb(color, &palette), tint)
}

fn tinted_rgb(color: [f32; 3], tint: eseqlisp::backend::Color) -> [f32; 3] {
    let weight = tint.a.clamp(0.0, 1.0);
    std::array::from_fn(|i| color[i] * (1.0 - weight) + [tint.r, tint.g, tint.b][i] * weight)
}

/// Snap `color` onto the theme's `:track-palette-N` entries: pick the set
/// (non-zero alpha) entry whose hue is nearest and blend toward it by that
/// entry's alpha. Colors too grey to have a hue blend toward the palette
/// entry nearest in lightness instead. No set entries: identity, so themes
/// without a palette keep the plain tint behaviour.
pub(crate) fn palette_snapped_rgb(
    color: [f32; 3],
    palette: &[eseqlisp::backend::Color],
) -> [f32; 3] {
    let set: Vec<&eseqlisp::backend::Color> = palette.iter().filter(|c| c.a > 0.0).collect();
    if set.is_empty() {
        return color;
    }
    let (hue, sat, light) = hsl(color);
    let pick = |metric: &dyn Fn(&eseqlisp::backend::Color) -> f32| {
        set.iter()
            .copied()
            .min_by(|a, b| metric(a).total_cmp(&metric(b)))
            .expect("non-empty palette")
    };
    let target = if sat < 0.12 {
        pick(&|c| (hsl([c.r, c.g, c.b]).2 - light).abs())
    } else {
        pick(&|c| {
            let d = (hsl([c.r, c.g, c.b]).0 - hue).abs();
            d.min(360.0 - d)
        })
    };
    let weight = target.a.clamp(0.0, 1.0);
    std::array::from_fn(|i| color[i] * (1.0 - weight) + [target.r, target.g, target.b][i] * weight)
}

/// (hue in degrees, saturation 0..1, lightness 0..1). Hue is 0 when grey.
fn hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, light);
    }
    let sat = delta / (1.0 - (2.0 * light - 1.0).abs()).max(f32::EPSILON);
    let hue = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (hue.rem_euclid(360.0), sat.min(1.0), light)
}

/// How `track.instrument-type` spells an instrument type.
pub(crate) fn instrument_type_label(
    instrument_type: sequencer::sequencer::InstrumentType,
) -> &'static str {
    match instrument_type {
        sequencer::sequencer::InstrumentType::Empty => "empty",
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
        sequencer::sequencer::InstrumentType::Rack => "rack",
    }
}

/// The id of the instrument `track` plays, in the Instruments tab's
/// `:instrument-id` form: the canonical saved-instrument id for custom
/// tracks, `builtin:<name>` for samplers and modulators, and "" otherwise
/// (empty tracks and racks, which the tab does not load in place):
/// `track.instrument-id`.
pub(crate) fn track_instrument_id(app: &app::App, track: usize) -> String {
    match app.graph.track_instrument_types.get(track) {
        Some(sequencer::sequencer::InstrumentType::Sampler) => {
            crate::browser::builtin_instrument_id("sampler")
        }
        Some(sequencer::sequencer::InstrumentType::Modulator) => {
            crate::browser::builtin_instrument_id("modulator")
        }
        Some(sequencer::sequencer::InstrumentType::Custom) => app
            .graph
            .track_engine_ids
            .get(track)
            .copied()
            .flatten()
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| crate::instrument_favorites::canonical_instrument_id(None, &engine.name))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Keep the host's track names cache (`LoopCtx::track_names`) on the
/// model's names.
pub(crate) fn refresh_track_names_cache(track_names: &mut Vec<String>, app: &app::App) {
    if *track_names != app.tracks {
        *track_names = app.tracks.clone();
    }
    crate::param_words::set_track_word_names(track_names);
}

/// The track's own send level to `bus` (0 without a send). `track` is in
/// range.
pub(crate) fn track_send_base(
    state: &SequencerState,
    track: usize,
    bus: sequencer::sequencer::BusId,
) -> f32 {
    state.pattern.track_params[track]
        .send_amount(bus)
        .unwrap_or(0.0)
}

/// The p-lock of `track`'s send to `bus` at `display_step`, if any.
pub(crate) fn track_send_lock(
    state: &SequencerState,
    track: usize,
    bus: sequencer::sequencer::BusId,
    display_step: Option<usize>,
) -> Option<f32> {
    display_step.and_then(|step| state.pattern.track_send_plocks[track].get(step, bus))
}

/// The send level shown at `display_step` (the host kinds' `send.display`):
/// its p-lock, else the base.
pub(crate) fn displayed_track_send_amount(
    state: &SequencerState,
    track: usize,
    bus: sequencer::sequencer::BusId,
    display_step: Option<usize>,
) -> f32 {
    track_send_lock(state, track, bus, display_step)
        .unwrap_or_else(|| track_send_base(state, track, bus))
}

/// The bus `bus` feeds (the main mix unless routed elsewhere; `bus.output`).
pub(crate) fn bus_output_destination(bus: &app::BusChannelState) -> sequencer::sequencer::BusId {
    sequencer::sequencer::BusId(bus.output.destination().unwrap_or(0))
}

