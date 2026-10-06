//! The `param` dyn word source (docs/jaki-plock-spec.md §5.2): the per-hit
//! parameter names a jaki `(plock NAME V)` can address on one track, for the
//! sexp-slot's `(dyn param)` atom (docs/sexp-slot-spec.md §4, §7).
//!
//! Context: the destination track index (`nil` or out of range: a single
//! hint row). Words are `ParamTarget::label()` spellings built from real
//! `ParamTarget`s, so completion and `ParamRef::parse` cannot drift; slot-free
//! spellings (`effect-param:…`, `midi-fx-param:…`, `instrument-param:…`) go
//! in `DynWords::aliases`: they validate, but the popup stays one row per
//! parameter.
//!
//! Only what the engine applies at landing is listed (see
//! `scheduler::params::apply_named_params`): instrument, effect and MIDI FX
//! params, the step params `step_param_from_target_name` accepts, and on
//! tracks with a rack its macros plus each slot's own params and instrument
//! params (`rack<N>:gain`, `rack<N>:instrument:cutoff`). Sends are skipped
//! there, so they are not offered.
//!
//! **Rails.** Every item (and alias) carries `DynItem::num`, the range a
//! `plock` value of that name scrubs, snaps and clamps to in the editor
//! (`(dyn-num param …)`, docs/jaki-plock-spec.md §7.1): the descriptor's own
//! min..max, the stored units the engine clamps to at landing, so the rails
//! agree with playback and with the detail text.
//!
//! **Data.** Words are built from the latest scheduler snapshot, the same
//! descriptors the engine resolves names against, so the source needs no
//! `App` and only locks the snapshot mutex for an `Arc` clone. Answers are
//! cached per track on the UI thread; [`refresh_param_word_source`] (run
//! every UI tick) re-fingerprints the cached tracks whenever a new snapshot
//! was published and bumps the source's epoch only when a track's parameter
//! set actually changed (instrument swap, FX / MIDI FX chain edit, rack
//! macros or slot instruments, track removal) — so open slots revalidate exactly then.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use eseqlisp::sexp_slot::dyn_words::{
    bump_dyn_word_epoch, query_dyn_words, register_dyn_word_source, DynGroup, DynItem, DynWords,
};
use eseqlisp::sexp_slot::schema::NumSpec;
use eseqlisp::vm::Value;
use eseqlisp::Runtime;
use sequencer::effects::{EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling};
use sequencer::process::{step_param_from_target_name, ParamTarget};
use sequencer::sequencer::{
    RackSlotParam, SequencerSnapshot, SequencerState, SequencerTrackSnapshot, StepParam,
};

/// The source name the jaki row schema spells as `(dyn "param")`.
pub(crate) const PARAM_WORD_SOURCE: &str = "param";

/// The `track` source (docs/jaki-row-processes-spec.md §13): every track as
/// its 0-based index, the number jaki routes and `(harmony :track n)` use,
/// with the track's name as the detail. Context-free.
pub(crate) const TRACK_WORD_SOURCE: &str = "track";

thread_local! {
    static TRACK_NAMES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Called wherever SEQ.track-names is published: a change bumps the `track`
/// source so open slots re-ask.
pub(crate) fn set_track_word_names(names: &[String]) {
    let changed = TRACK_NAMES.with(|stored| {
        let mut stored = stored.borrow_mut();
        if stored.as_slice() == names {
            return false;
        }
        *stored = names.to_vec();
        true
    });
    if changed {
        bump_dyn_word_epoch(TRACK_WORD_SOURCE);
    }
}

fn track_words() -> DynWords {
    TRACK_NAMES.with(|names| DynWords {
        epoch: 0,
        groups: vec![DynGroup {
            group: String::new(),
            items: names
                .borrow()
                .iter()
                .enumerate()
                .map(|(index, name)| DynItem::new(index.to_string(), format!("{} {name}", index + 1)))
                .collect(),
        }],
        aliases: Vec::new(),
    })
}

/// Shown (dim, unselectable) when a row has no destination track.
pub(crate) const UNROUTED_HINT: &str = "route this row to see its parameters";

/// Step params in the order they are offered, with the spelling used in the
/// `step-param:` word. Every one must resolve through
/// `step_param_from_target_name` (sync / delay do not: not per hit).
const STEP_WORDS: [(StepParam, &str); 10] = [
    (StepParam::Velocity, "velocity"),
    (StepParam::Duration, "duration"),
    (StepParam::Transpose, "transpose"),
    (StepParam::Pan, "pan"),
    (StepParam::Speed, "speed"),
    (StepParam::AuxA, "aux-a"),
    (StepParam::AuxB, "aux-b"),
    (StepParam::Chop, "chop"),
    (StepParam::Retrig, "retrig"),
    (StepParam::RetrigRate, "retrig-rate"),
];

/// The step params a process port may write, by their canonical
/// `step-param` target names (`project.step-param-options`).
pub(crate) fn step_param_target_names() -> impl Iterator<Item = &'static str> {
    (STEP_WORDS.iter())
        .filter(|(param, name)| step_param_from_target_name(name) == Some(*param))
        .map(|(_, name)| *name)
}

/// The canonical `step-param` target name of the step param `name` names
/// (any spelling `step_param_from_target_name` accepts), if it is one a
/// process port may write.
pub(crate) fn canonical_step_param_name(name: &str) -> Option<&'static str> {
    let param = step_param_from_target_name(name)?;
    step_param_target_names().find(|known| step_param_from_target_name(known) == Some(param))
}

struct CachedTrack {
    fingerprint: u64,
    words: Rc<DynWords>,
}

struct ParamWordSource {
    state: Arc<SequencerState>,
    /// Scheduler snapshot version the cache was last checked against.
    checked_version: u64,
    tracks: HashMap<usize, CachedTrack>,
}

thread_local! {
    static SOURCE: RefCell<Option<ParamWordSource>> = const { RefCell::new(None) };
}

/// Register the `param` source (UI thread) and the `dyn-word-valid?` native.
pub(crate) fn register_param_word_source(runtime: &mut Runtime, state: Arc<SequencerState>) {
    let checked_version = state.scheduler_snapshot_version();
    SOURCE.with(|source| {
        *source.borrow_mut() = Some(ParamWordSource {
            state,
            checked_version,
            tracks: HashMap::new(),
        });
    });
    register_dyn_word_source(PARAM_WORD_SOURCE, Box::new(|context| param_words(context).as_ref().clone()));
    register_dyn_word_source(TRACK_WORD_SOURCE, Box::new(|_context| track_words()));
    runtime.register_native_with_docs(
        "dyn-word-valid?",
        "(dyn-word-valid? source context word)",
        "Whether the host dyn word source offers WORD for CONTEXT (a sexp-slot \
         `(dyn SOURCE)` atom's validity): nil when CONTEXT is nil or SOURCE is \
         not registered.",
        |args, _ctx| {
            let (Some(Value::String(source)), Some(context), Some(word)) =
                (args.first(), args.get(1), args.get(2))
            else {
                return Err("dyn-word-valid? expects (source context word)".to_string());
            };
            if matches!(context, Value::Nil) {
                return Ok(Value::Nil);
            }
            let word = match word {
                Value::String(s) | Value::Symbol(s) | Value::Keyword(s) => s.clone(),
                _ => return Ok(Value::Bool(false)),
            };
            Ok(match query_dyn_words(source, context) {
                Some(words) => Value::Bool(words.contains(&word)),
                None => Value::Nil,
            })
        },
    );
}

/// The words for one context, from the per-track cache or freshly built.
fn param_words(context: &Value) -> Rc<DynWords> {
    let track = match context {
        Value::Number(n) if n.is_finite() && *n >= 0.0 && n.fract() == 0.0 => *n as usize,
        _ => return Rc::new(hint_words()),
    };
    SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        let Some(source) = source.as_mut() else {
            return Rc::new(hint_words());
        };
        if let Some(cached) = source.tracks.get(&track) {
            return cached.words.clone();
        }
        let snapshot = source.state.latest_scheduler_snapshot();
        let Some(track_snapshot) = snapshot.tracks.get(track) else {
            return Rc::new(hint_words());
        };
        let words = Rc::new(build_track_param_words(&snapshot, track_snapshot));
        source.tracks.insert(
            track,
            CachedTrack {
                fingerprint: track_fingerprint(&snapshot, track_snapshot),
                words: words.clone(),
            },
        );
        words
    })
}

/// Called every UI tick: when a new scheduler snapshot was published, drop
/// the cached tracks whose parameter set changed and bump the `param` epoch
/// so open slots revalidate. Returns whether the epoch moved. Cheap when
/// nothing was asked yet (no cache) or nothing was published.
pub(crate) fn refresh_param_word_source() -> bool {
    let changed = SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        let Some(source) = source.as_mut() else {
            return false;
        };
        let version = source.state.scheduler_snapshot_version();
        if version == source.checked_version {
            return false;
        }
        source.checked_version = version;
        if source.tracks.is_empty() {
            return false;
        }
        let snapshot = source.state.latest_scheduler_snapshot();
        let before = source.tracks.len();
        source.tracks.retain(|track, cached| {
            snapshot
                .tracks
                .get(*track)
                .is_some_and(|track| track_fingerprint(&snapshot, track) == cached.fingerprint)
        });
        source.tracks.len() != before
    });
    if changed {
        bump_dyn_word_epoch(PARAM_WORD_SOURCE);
    }
    changed
}

fn hint_words() -> DynWords {
    DynWords {
        epoch: 0,
        groups: vec![DynGroup {
            group: String::new(),
            items: vec![DynItem::new("", UNROUTED_HINT)],
        }],
        aliases: Vec::new(),
    }
}

/// Everything that decides a track's words: descriptor names, param names
/// and ranges, the MIDI FX chain names, the rack macros and the rack slots'
/// instrument descriptors.
fn track_fingerprint(snapshot: &SequencerSnapshot, track: &SequencerTrackSnapshot) -> u64 {
    fn hash_desc(desc: &EffectDescriptor, hasher: &mut DefaultHasher) {
        desc.name.hash(hasher);
        desc.params.len().hash(hasher);
        for param in &desc.params {
            param.name.hash(hasher);
            param.min.to_bits().hash(hasher);
            param.max.to_bits().hash(hasher);
            param_kind_key(&param.kind).hash(hasher);
            display_name(param).hash(hasher);
        }
    }
    let mut hasher = DefaultHasher::new();
    hash_desc(&track.instrument_descriptor, &mut hasher);
    track.effect_descriptors.len().hash(&mut hasher);
    for desc in &track.effect_descriptors {
        hash_desc(desc, &mut hasher);
    }
    track.params.midi_fx_chain.hash(&mut hasher);
    match &track.rack_track {
        Some(rack) => {
            rack.macros.len().hash(&mut hasher);
            for rack_macro in &rack.macros {
                rack_macro.name.hash(&mut hasher);
            }
            rack.slots.len().hash(&mut hasher);
            for slot in &rack.slots {
                match snapshot.rack_slot_instrument_descriptor(slot) {
                    Some(desc) => hash_desc(desc, &mut hasher),
                    None => usize::MAX.hash(&mut hasher),
                }
            }
        }
        None => usize::MAX.hash(&mut hasher),
    }
    hasher.finish()
}

fn param_kind_key(kind: &ParamKind) -> String {
    match kind {
        ParamKind::Continuous { unit } => format!("c{}", unit.as_deref().unwrap_or("")),
        ParamKind::Boolean => "b".to_string(),
        ParamKind::Enum { labels } => format!("e{}", labels.join("\u{1f}")),
    }
}

fn display_name(param: &ParamDescriptor) -> &str {
    param
        .ui_metadata
        .as_ref()
        .and_then(|meta| meta.display_name.as_deref())
        .filter(|name| !name.is_empty())
        .unwrap_or(&param.name)
}

/// A number as short text: integers bare, else up to three decimals with
/// trailing zeros trimmed.
fn short_number(value: f32) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e9 {
        return format!("{}", value as i64);
    }
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `Cutoff · 20–20000 Hz`: the knob's name and the range a `plock` value is
/// clamped to (the engine clamps to the descriptor's own min..max, so a
/// percent knob's values are 0–1 here, not 0–100).
fn param_detail(param: &ParamDescriptor) -> String {
    let (lo, hi) = if param.min <= param.max {
        (param.min, param.max)
    } else {
        (param.max, param.min)
    };
    let range = match &param.kind {
        ParamKind::Boolean => "0–1 off/on".to_string(),
        ParamKind::Enum { labels } => {
            let shown: Vec<&str> = labels.iter().take(4).map(String::as_str).collect();
            let more = if labels.len() > shown.len() { " …" } else { "" };
            format!("{}–{} {}{more}", short_number(lo), short_number(hi), shown.join("/"))
        }
        ParamKind::Continuous { unit } => match unit.as_deref() {
            Some(unit) if !unit.is_empty() && unit != "%" => {
                format!("{}–{} {unit}", short_number(lo), short_number(hi))
            }
            _ => format!("{}–{}", short_number(lo), short_number(hi)),
        },
    };
    format!("{} · {range}", display_name(param))
}

fn item(target: ParamTarget, detail: String, num: NumSpec) -> DynItem {
    DynItem::new(target.label(), detail).with_num(num)
}

/// An alias: valid, never offered, same rails as the word it spells.
fn alias(word: String, num: NumSpec) -> DynItem {
    DynItem::new(word, "").with_num(num)
}

/// The editor rails of a descriptor param, in the units the engine clamps
/// to (a percent knob's stored 0..1, not the knob's 0..100; eseq-jplk.7).
/// Enums and booleans step by 1 with no decimals. A continuous param steps
/// by a power of ten about a hundredth of its span (at most 1), or for an
/// exponential (frequency-like) one a tenth of its bottom decade, so a
/// 20..20000 Hz cutoff types whole Hz and a 0..1 amount two decimals.
pub(crate) fn param_num_spec(param: &ParamDescriptor) -> NumSpec {
    let (lo, hi) = if param.min <= param.max {
        (param.min as f64, param.max as f64)
    } else {
        (param.max as f64, param.min as f64)
    };
    let span = hi - lo;
    let exponent = match &param.kind {
        ParamKind::Boolean | ParamKind::Enum { .. } => 0,
        ParamKind::Continuous { .. } if !(span > 0.0) || !span.is_finite() => -2,
        ParamKind::Continuous { .. } => {
            let decade = match param.scaling {
                ParamScaling::Exponential if lo > 0.0 => lo.log10().floor() as i32 - 1,
                _ => span.log10().floor() as i32 - 2,
            };
            decade.clamp(-4, 0)
        }
    };
    NumSpec {
        min: lo,
        max: hi,
        step: 10f64.powi(exponent),
        decimals: exponent.unsigned_abs(),
    }
}

/// Step-param rails: whole units where the step lane nudges by whole units,
/// else hundredths.
fn step_num_spec(param: StepParam) -> NumSpec {
    let whole = param.increment() >= 1.0;
    NumSpec {
        min: param.min() as f64,
        max: param.max() as f64,
        step: if whole { 1.0 } else { 0.01 },
        decimals: if whole { 0 } else { 2 },
    }
}

/// Rack macros are 0..1.
const MACRO_NUM_SPEC: NumSpec = NumSpec {
    min: 0.0,
    max: 1.0,
    step: 0.01,
    decimals: 2,
};

/// The editor rails of a rack slot's own param, matching
/// `RackSlotParam::clamp`.
fn rack_slot_param_num_spec(param: RackSlotParam) -> NumSpec {
    let (min, max, step, decimals) = match param {
        RackSlotParam::BaseNote => (-48.0, 48.0, 1.0, 0),
        RackSlotParam::Gain => (0.0, 2.0, 0.01, 2),
        RackSlotParam::Pan => (-1.0, 1.0, 0.01, 2),
        RackSlotParam::MaxPolyphony => (1.0, sequencer::audio::MAX_VOICES as f64, 1.0, 0),
        RackSlotParam::Mute | RackSlotParam::Solo => (0.0, 1.0, 1.0, 0),
    };
    NumSpec { min, max, step, decimals }
}

fn rack_slot_param_detail(param: RackSlotParam) -> String {
    let spec = rack_slot_param_num_spec(param);
    let label = match param {
        RackSlotParam::BaseNote => "Base note",
        RackSlotParam::Gain => "Gain",
        RackSlotParam::Pan => "Pan",
        RackSlotParam::MaxPolyphony => "Polyphony",
        RackSlotParam::Mute => "Mute",
        RackSlotParam::Solo => "Solo",
    };
    let range = format!(
        "{}–{}",
        short_number(spec.min as f32),
        short_number(spec.max as f32)
    );
    match param {
        RackSlotParam::Mute | RackSlotParam::Solo => format!("{label} · {range} off/on"),
        _ => format!("{label} · {range}"),
    }
}

/// The groups + aliases for one track, from its scheduler snapshot.
pub(crate) fn build_track_param_words(
    snapshot: &SequencerSnapshot,
    track: &SequencerTrackSnapshot,
) -> DynWords {
    let mut groups = Vec::new();
    let mut aliases = Vec::new();

    let inst = &track.instrument_descriptor;
    if !inst.params.is_empty() {
        groups.push(DynGroup {
            group: if inst.name.is_empty() {
                "Instrument".to_string()
            } else {
                inst.name.clone()
            },
            items: inst
                .params
                .iter()
                .map(|param| {
                    let num = param_num_spec(param);
                    aliases.push(alias(format!("instrument-param:{}", param.name), num));
                    item(
                        ParamTarget::InstrumentParam {
                            param: param.name.clone(),
                            param_id: None,
                        },
                        param_detail(param),
                        num,
                    )
                })
                .collect(),
        });
    }

    for (slot, desc) in track.effect_descriptors.iter().enumerate() {
        if desc.params.is_empty() {
            continue;
        }
        groups.push(DynGroup {
            group: format!("FX {} · {}", slot + 1, desc.name),
            items: desc
                .params
                .iter()
                .map(|param| {
                    let num = param_num_spec(param);
                    aliases.push(alias(format!("effect-param:{}:{}", desc.name, param.name), num));
                    item(
                        ParamTarget::EffectParam {
                            slot,
                            effect: desc.name.clone(),
                            param: param.name.clone(),
                            param_id: None,
                        },
                        param_detail(param),
                        num,
                    )
                })
                .collect(),
        });
    }

    for (slot, fx) in track.params.midi_fx_chain.iter().enumerate() {
        let Some(desc) = sequencer::lisp_host::load_midi_fx_descriptor(fx) else {
            continue;
        };
        if desc.params.is_empty() {
            continue;
        }
        groups.push(DynGroup {
            group: format!("MIDI FX {} · {}", slot + 1, desc.name),
            items: desc
                .params
                .iter()
                .map(|param| {
                    // The chain entry's name: what the engine matches the
                    // slot against (case-insensitive).
                    let num = param_num_spec(param);
                    aliases.push(alias(format!("midi-fx-param:{fx}:{}", param.name), num));
                    item(
                        ParamTarget::MidiFxParam {
                            slot,
                            fx: fx.clone(),
                            param: param.name.clone(),
                        },
                        param_detail(param),
                        num,
                    )
                })
                .collect(),
        });
    }

    if let Some(rack) = &track.rack_track {
        // The engine addresses macros by position (`rack.macros.get(i)`).
        let items: Vec<DynItem> = rack
            .macros
            .iter()
            .enumerate()
            .filter_map(|(index, rack_macro)| {
                let macro_id = u8::try_from(index).ok()?;
                Some(item(
                    ParamTarget::RackMacroParam { macro_id },
                    format!("{} · 0–1", rack_macro.name),
                    MACRO_NUM_SPEC,
                ))
            })
            .collect();
        if !items.is_empty() {
            groups.push(DynGroup {
                group: "Macros".to_string(),
                items,
            });
        }

        // Each slot: its instrument's params, then its own mixer params.
        for (slot, rack_slot) in rack.slots.iter().enumerate() {
            let desc = snapshot.rack_slot_instrument_descriptor(rack_slot);
            let mut items: Vec<DynItem> = desc
                .map(|desc| {
                    desc.params
                        .iter()
                        .take(rack_slot.instrument_slot.defaults.len())
                        .map(|param| {
                            item(
                                ParamTarget::RackSlotInstrumentParam {
                                    slot,
                                    param: param.name.clone(),
                                    param_id: None,
                                },
                                param_detail(param),
                                param_num_spec(param),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            items.extend(RackSlotParam::ALL.into_iter().map(|param| {
                item(
                    ParamTarget::RackSlotParam {
                        slot,
                        param: param.name().to_string(),
                    },
                    rack_slot_param_detail(param),
                    rack_slot_param_num_spec(param),
                )
            }));
            groups.push(DynGroup {
                group: match desc.filter(|desc| !desc.name.is_empty()) {
                    Some(desc) => format!("Slot {} · {}", slot + 1, desc.name),
                    None => format!("Slot {}", slot + 1),
                },
                items,
            });
        }
    }

    groups.push(DynGroup {
        group: "Step".to_string(),
        items: STEP_WORDS
            .iter()
            .filter(|(param, name)| step_param_from_target_name(name) == Some(*param))
            .map(|(param, name)| {
                item(
                    ParamTarget::StepParam {
                        param: (*name).to_string(),
                    },
                    format!(
                        "{} · {}–{}",
                        param.label(),
                        short_number(param.min()),
                        short_number(param.max())
                    ),
                    step_num_spec(*param),
                )
            })
            .collect(),
    });

    DynWords {
        epoch: 0,
        groups,
        aliases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::sexp_slot::dyn_words::dyn_word_epoch;
    use sequencer::effects::EffectDescriptor;
    use sequencer::sequencer::default_empty_effect_chain;
    use sequencer::process::ParamRef;

    #[test]
    fn track_word_source_lists_tracks_only_never_bus_names() {
        register_dyn_word_source(TRACK_WORD_SOURCE, Box::new(|_context| track_words()));
        crate::state_values::build_track_names(&["Spectral".to_string(), "Digi FM".to_string()]);
        // the mixer publishes bus names through its own list builder
        crate::state_values::build_name_list(&["Mix".to_string(), "Bus A".to_string()]);
        let words = query_dyn_words(TRACK_WORD_SOURCE, &Value::Nil).expect("track source");
        let items: Vec<(String, String)> = words
            .items()
            .map(|(_, item)| (item.word.clone(), item.detail.clone()))
            .collect();
        assert_eq!(
            items,
            [("0".to_string(), "1 Spectral".to_string()), ("1".to_string(), "2 Digi FM".to_string())]
        );
    }

    fn state_with_sampler_and_filter() -> Arc<SequencerState> {
        let state = Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| default_empty_effect_chain()).collect(),
        ));
        let sampler = EffectDescriptor::builtin_sampler();
        let filter = EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&sampler, 12);
        state.pattern.effect_chains[0][0].apply_descriptor(&filter, 42);
        let mut effects = vec![EffectDescriptor::default_full_chain(); 2];
        effects[0][0] = filter;
        state.set_scratch_runtime_descriptors(effects, vec![sampler; 2]);
        state
    }

    fn group<'a>(words: &'a DynWords, name: &str) -> &'a DynGroup {
        words
            .groups
            .iter()
            .find(|group| group.group == name)
            .unwrap_or_else(|| {
                panic!(
                    "no group {name:?} in {:?}",
                    words.groups.iter().map(|g| &g.group).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn param_words_list_instrument_fx_and_step_labels_for_a_track() {
        let state = state_with_sampler_and_filter();
        let mut runtime = Runtime::new();
        register_param_word_source(&mut runtime, Arc::clone(&state));
        let words = param_words(&Value::Number(0.0));

        let sampler = group(&words, &EffectDescriptor::builtin_sampler().name);
        let speed = sampler
            .items
            .iter()
            .find(|item| item.word == "instrument:speed")
            .expect("sampler speed listed");
        assert!(speed.detail.contains(" · "), "detail {:?}", speed.detail);

        let filter_name = EffectDescriptor::builtin_filter().name;
        let fx = group(&words, &format!("FX 1 · {filter_name}"));
        let mode = format!("fx1:{filter_name}:mode");
        assert!(fx.items.iter().any(|item| item.word == mode), "{:?}", fx.items);
        assert!(words.contains(&format!("effect-param:{filter_name}:mode")));
        assert!(words.contains("instrument-param:speed"));
        // aliases validate but are never offered
        assert!(!words.items().any(|(_, item)| item.word.starts_with("effect-param:")));

        let step = group(&words, "Step");
        let step_words: Vec<&str> = step.items.iter().map(|item| item.word.as_str()).collect();
        assert!(step_words.contains(&"step-param:pan"));
        assert!(step_words.contains(&"step-param:retrig-rate"));
        assert!(!step_words.iter().any(|w| w.contains("sync") || w.contains("delay")));

        // every offered word parses back (display and parser cannot drift)
        for (_, item) in words.items() {
            ParamRef::parse(&item.word)
                .unwrap_or_else(|err| panic!("{} does not parse: {err}", item.word));
        }
        for alias in &words.aliases {
            ParamRef::parse(&alias.word).unwrap_or_else(|err| panic!("{}: {err}", alias.word));
        }
        // no rack on this track: no macros
        assert!(!words.groups.iter().any(|group| group.group == "Macros"));
    }

    #[test]
    fn param_words_carry_number_rails_for_items_and_aliases() {
        let state = state_with_sampler_and_filter();
        let snapshot = state.latest_scheduler_snapshot();
        let words = build_track_param_words(&snapshot, &snapshot.tracks[0]);
        let filter_name = EffectDescriptor::builtin_filter().name;
        let spec = |word: &str| words.num_spec(word).unwrap_or_else(|| panic!("no rails for {word}"));
        let rails = |spec: NumSpec| (spec.min, spec.max, spec.step, spec.decimals);

        // Exponential Hz: whole Hz across the descriptor's own range.
        let cutoff = spec(&format!("fx1:{filter_name}:cutoff"));
        assert_eq!(rails(cutoff), (20.0, 20000.0, 1.0, 0));
        // An enum steps by one, no decimals.
        assert_eq!(rails(spec(&format!("fx1:{filter_name}:mode"))), (0.0, 3.0, 1.0, 0));
        // Linear continuous: hundredths of a small span; a percent knob in its
        // stored 0..1 (what the engine clamps to), not the knob's 0..100.
        assert_eq!(rails(spec(&format!("fx1:{filter_name}:resonance"))), (0.5, 10.0, 0.01, 2));
        assert_eq!(rails(spec(&format!("fx1:{filter_name}:drive"))), (0.0, 1.0, 0.01, 2));
        // The slot-free alias resolves to the same rails.
        assert_eq!(words.num_spec(&format!("effect-param:{filter_name}:cutoff")), Some(cutoff));
        // Every offered word and alias has rails.
        for (_, item) in words.items() {
            assert!(item.num.is_some(), "{} has no rails", item.word);
        }
        assert!(words.aliases.iter().all(|alias| alias.num.is_some()));
        // Step params: whole-unit lanes step by 1, others by hundredths.
        assert_eq!(rails(spec("step-param:transpose")), (-48.0, 48.0, 1.0, 0));
        assert_eq!(rails(spec("step-param:pan")), (-1.0, 1.0, 0.01, 2));

        // A boolean is 0..1 in whole steps.
        let mut flag = EffectDescriptor::builtin_filter().params[0].clone();
        flag.kind = ParamKind::Boolean;
        flag.min = 0.0;
        flag.max = 1.0;
        assert_eq!(rails(param_num_spec(&flag)), (0.0, 1.0, 1.0, 0));
    }

    #[test]
    fn param_words_list_rack_macros_on_a_rack_track() {
        let state = state_with_sampler_and_filter();
        state.set_rack_track_for_all_pattern_snapshots(
            1,
            sequencer::sequencer::RackTrackSnapshot::new(
                Vec::new(),
                sequencer::sequencer::default_rack_macros(),
            ),
        );
        state.publish_scheduler_snapshot();
        let snapshot = state.latest_scheduler_snapshot();
        let words = build_track_param_words(&snapshot, &snapshot.tracks[1]);
        let macros = group(&words, "Macros");
        assert_eq!(macros.items[0].word, "rack-macro:macro_1");
        assert_eq!(macros.items[0].num, Some(MACRO_NUM_SPEC));
        assert!(macros.items[0].detail.starts_with("Macro 1"));
    }

    #[test]
    fn param_words_list_each_rack_slots_instrument_and_mixer_params() {
        use sequencer::effects::EffectSlotSnapshot;
        use sequencer::sequencer::{
            CustomInstrumentRunMode, InstrumentType, RackSlotParamPlocks, RackSlotSnapshot,
            RackTrackSnapshot, TrackSoundState,
        };
        let state = state_with_sampler_and_filter();
        let filter = EffectDescriptor::builtin_filter();
        let slot = |instrument_type, desc: &EffectDescriptor| RackSlotSnapshot {
            instrument_type,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony: 2,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default(desc, 9),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: None,
        };
        let mut synth = slot(InstrumentType::Custom, &filter);
        synth.track_sound_state.engine_id = Some(0);
        state.set_rack_track_for_all_pattern_snapshots(
            1,
            RackTrackSnapshot::new(
                vec![synth, slot(InstrumentType::Sampler, &EffectDescriptor::builtin_sampler())],
                sequencer::sequencer::default_rack_macros(),
            ),
        );
        state.sync_engine_instrument_descriptors(1, || vec![filter.clone()]);
        let snapshot = state.latest_scheduler_snapshot();
        let words = build_track_param_words(&snapshot, &snapshot.tracks[1]);

        let first = group(&words, &format!("Slot 1 · {}", filter.name));
        let mode = format!("rack1:instrument:{}", filter.params[0].name);
        let mode_item = first.items.iter().find(|item| item.word == mode).expect("slot 1 param");
        assert_eq!(mode_item.num, Some(param_num_spec(&filter.params[0])));
        assert!(first.items.iter().any(|item| item.word == "rack1:gain"));
        let second = group(
            &words,
            &format!("Slot 2 · {}", EffectDescriptor::builtin_sampler().name),
        );
        assert!(second.items.iter().any(|item| item.word == "rack2:instrument:speed"));
        // Every offered word parses to the rack-slot reference it names.
        for item in first.items.iter().chain(&second.items) {
            assert!(
                matches!(
                    ParamRef::parse(&item.word),
                    Ok(ParamRef::RackSlot { .. } | ParamRef::RackSlotInstrument { .. })
                ),
                "{}",
                item.word
            );
        }
    }

    #[test]
    fn param_words_hint_for_unrouted_or_missing_tracks() {
        let state = state_with_sampler_and_filter();
        let mut runtime = Runtime::new();
        register_param_word_source(&mut runtime, state);
        for context in [Value::Nil, Value::Number(-1.0), Value::Number(99.0)] {
            let words = param_words(&context);
            assert_eq!(words.hints().collect::<Vec<_>>(), vec![UNROUTED_HINT]);
            assert!(!words.items().any(|(_, item)| !item.word.is_empty()));
        }
    }

    #[test]
    fn param_word_epoch_advances_when_the_fx_chain_changes() {
        let state = state_with_sampler_and_filter();
        let mut runtime = Runtime::new();
        register_param_word_source(&mut runtime, Arc::clone(&state));
        let filter_name = EffectDescriptor::builtin_filter().name;
        let mode = format!("fx1:{filter_name}:mode");
        fn valid(runtime: &mut Runtime, word: &str) -> Value {
            runtime
                .eval_str(&format!("(dyn-word-valid? \"param\" 0 \"{word}\")"))
                .expect("eval")
                .unwrap_or(Value::Nil)
        }
        assert_eq!(valid(&mut runtime, &mode), Value::Bool(true));
        let epoch = dyn_word_epoch(PARAM_WORD_SOURCE).expect("registered");

        // A publish that changes nothing structural keeps the epoch.
        state.publish_scheduler_snapshot();
        assert!(!refresh_param_word_source());
        assert_eq!(dyn_word_epoch(PARAM_WORD_SOURCE), Some(epoch));

        // Removing the filter from track 0's chain: epoch moves, name invalid.
        state.set_scratch_runtime_descriptors(
            vec![EffectDescriptor::default_full_chain(); 2],
            vec![EffectDescriptor::builtin_sampler(); 2],
        );
        assert!(refresh_param_word_source());
        assert!(dyn_word_epoch(PARAM_WORD_SOURCE).expect("registered") > epoch);
        assert_eq!(valid(&mut runtime, &mode), Value::Bool(false));
        // nil context: never judged
        assert_eq!(
            runtime
                .eval_str(&format!("(dyn-word-valid? \"param\" nil \"{mode}\")"))
                .expect("eval")
                .unwrap_or(Value::Nil),
            Value::Nil
        );
    }
}
