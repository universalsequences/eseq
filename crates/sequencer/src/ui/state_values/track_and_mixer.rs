use super::*;

/// Build a Lisp Value::List of bools for record-armed state per track.
pub(crate) fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
pub(crate) fn build_track_names(names: &[String]) -> Value {
    // every SEQ.track-names publish also refreshes the sexp-slot `track`
    // word source (jaki's `(harmony :track n)` dropdown) — so only TRACK
    // names may come through here; other name lists use build_name_list
    crate::param_words::set_track_word_names(names);
    build_name_list(names)
}

/// A Lisp list of name strings (bus names and the like), no side effects.
pub(crate) fn build_name_list(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

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

/// Republish the color projections together on a theme switch.
pub(crate) fn sync_track_color_state(rt: &mut Runtime, app: &app::App) {
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
}

pub(crate) fn build_track_colors(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| {
            let color = track_display_color(app, track);
            Rc::new(RefCell::new(Value::List(vec![
                Rc::new(RefCell::new(Value::Number(color.r as f64))),
                Rc::new(RefCell::new(Value::Number(color.g as f64))),
                Rc::new(RefCell::new(Value::Number(color.b as f64))),
            ])))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_ids(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_node_ids
        .iter()
        .map(|ids| Rc::new(RefCell::new(Value::Number(ids.pan_id as f64))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_types(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_types
        .iter()
        .map(|instrument_type| {
            let label = instrument_type_label(*instrument_type);
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

/// `SEQ.track-instrument-types` and `track.instrument-type` spell an
/// instrument type this way.
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

pub(crate) fn build_track_instrument_run_modes(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_run_modes
        .iter()
        .map(|run_mode| {
            let label = match run_mode {
                sequencer::sequencer::CustomInstrumentRunMode::Instrument => "instrument",
                sequencer::sequencer::CustomInstrumentRunMode::FreePatch => "free_patch",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_name_state(
    rt: &mut Runtime,
    track_names: &mut Vec<String>,
    app: &app::App,
) {
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    if *track_names != app.tracks {
        *track_names = app.tracks.clone();
    }
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
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

pub(crate) fn build_track_muted_by_solo(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(track_muted_by_solo(
                state, t, &solo,
            ))))
        })
        .collect();
    Value::List(items)
}

pub(super) fn track_effectively_muted(
    state: &Arc<SequencerState>,
    track: usize,
    solo: &app::SoloAudibility,
) -> bool {
    let params = &state.pattern.track_params[track];
    params.is_muted() || solo.track_is_muted(params)
}

pub(super) fn track_muted_by_solo(
    state: &Arc<SequencerState>,
    track: usize,
    solo: &app::SoloAudibility,
) -> bool {
    solo.track_is_muted(&state.pattern.track_params[track])
}

/// Per-track "effectively muted" (explicit mute OR muted by another track's
/// solo) as 0/1 numbers, for widget bindings via `bind-seq-nth`. Lets row
/// `:muted` props update without rerunning the row's subtree.
pub(crate) fn build_track_muted_effective(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let muted = track_effectively_muted(state, t, &solo);
            Rc::new(RefCell::new(Value::Number(if muted { 1.0 } else { 0.0 })))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_mute_visual_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    tracks: impl IntoIterator<Item = usize>,
    sync_muted_by_solo: bool,
) -> bool {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let mut effects_dirty = false;

    for track in tracks {
        if track >= count {
            continue;
        }
        if sync_muted_by_solo {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    "track-muted-by-solo",
                    track,
                    Value::Bool(track_muted_by_solo(state, track, &solo)),
                )
                .effects_dirty;
        }

        let muted = track_effectively_muted(state, track, &solo);
        effects_dirty |= rt
            .set_reactive_list_index(
                "SEQ",
                "track-muted-effective",
                track,
                Value::Number(if muted { 1.0 } else { 0.0 }),
            )
            .effects_dirty;
    }

    effects_dirty
}

pub(crate) fn sync_track_mixer_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    // Compact channel views list devices by name; this rides the same sync
    // as the instrument types so any mixer refresh (project load, effect
    // add/remove, dgen compile landing) carries the chain too.
    rt.set_reactive(
        "SEQ",
        "track-device-chains",
        build_track_device_chains_value(app, state),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-muted-by-solo",
        build_track_muted_by_solo(app, state),
    );
    rt.set_reactive(
        "SEQ",
        "track-muted-effective",
        build_track_muted_effective(app, state),
    );
}

/// The bus `bus` feeds (the main mix unless routed elsewhere; `bus.output`).
pub(crate) fn bus_output_destination(bus: &app::BusChannelState) -> sequencer::sequencer::BusId {
    sequencer::sequencer::BusId(bus.output.destination().unwrap_or(0))
}

pub(crate) fn sync_bus_mixer_control_state(rt: &mut Runtime, app: &app::App) {
    let names: Vec<String> = app.buses.iter().map(|bus| bus.name.clone()).collect();
    rt.set_reactive("SEQ", "bus-names", build_name_list(&names));
}

pub(crate) fn sync_bus_mixer_state(rt: &mut Runtime, app: &app::App) {
    sync_bus_mixer_control_state(rt, app);
    rt.set_reactive("SEQ", "bus-device-chains", build_bus_device_chains_value(app));
}

pub(crate) fn sync_track_mixer_empty_state(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "track-colors", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-instrument-types", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-by-solo", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-names", Value::List(vec![]));
}
