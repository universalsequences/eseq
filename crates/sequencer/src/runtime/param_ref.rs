//! Text names for per-hit parameter targets (docs/jaki-plock-spec.md §2).
//!
//! [`ParamTarget::label`] is the one spelling the macro editor shows for a
//! mapping target; [`ParamRef::parse`] reads that spelling (plus the
//! slot-free `ProcessTargetHint` spellings) back into an **unresolved**
//! reference. It stays unresolved because the track a jaki row plays is not
//! known when the pattern is written: resolution against a destination
//! track's instrument / chain happens at landing, by name.

use super::process::{bus_send_label, ParamTarget};
use crate::sequencer::{StepParam, DEFAULT_BUS_A_ID, DEFAULT_BUS_B_ID};

/// A parameter named by its label, not yet resolved against any track.
///
/// Slots are 0-based here and 1-based in the text. `effect` / `fx` keep the
/// case they were written in; matching against descriptor names is
/// case-insensitive at resolution time. `slot: None` is the slot-free
/// spelling (`effect-param:…`, `midi-fx-param:…`): the first effect / MIDI
/// FX of that name in the destination's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamRef {
    Instrument {
        param: String,
    },
    Effect {
        slot: Option<usize>,
        effect: String,
        param: String,
    },
    MidiFx {
        slot: Option<usize>,
        fx: String,
        param: String,
    },
    Step {
        param: StepParam,
    },
    RackSlot {
        slot: usize,
        param: String,
    },
    RackSlotInstrument {
        slot: usize,
        param: String,
    },
    RackMacro {
        macro_idx: usize,
    },
    Send {
        bus: u64,
    },
}

impl ParamTarget {
    /// The macro editor's display label for this target; the text form
    /// [`ParamRef::parse`] reads back (all variants but `ProcessInlet`).
    pub fn label(&self) -> String {
        match self {
            ParamTarget::StepParam { param } => format!("step-param:{param}"),
            ParamTarget::InstrumentParam { param, .. } => format!("instrument:{param}"),
            ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            } => format!("fx{}:{effect}:{param}", slot + 1),
            ParamTarget::MidiFxParam { slot, fx, param } => {
                format!("midi-fx{}:{fx}:{param}", slot + 1)
            }
            ParamTarget::ProcessInlet {
                process,
                inlet,
                instance_id,
            } => instance_id
                .map(|id| format!("process:{process}#{}:{inlet}", id.0))
                .unwrap_or_else(|| format!("process:{process}:{inlet}")),
            ParamTarget::RackSlotParam { slot, param } => format!("rack{}:{param}", slot + 1),
            ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
                format!("rack{}:instrument:{param}", slot + 1)
            }
            ParamTarget::RackMacroParam { macro_id } => {
                format!("rack-macro:macro_{}", macro_id + 1)
            }
            ParamTarget::BusSend { bus } => format!("send:{}", bus_send_label(*bus)),
        }
    }
}

/// Inverse of [`bus_send_label`]: `A` / `B` are the default buses, `busN`
/// is bus id `N`.
pub fn bus_send_from_label(label: &str) -> Option<u64> {
    match label {
        "A" | "a" => Some(DEFAULT_BUS_A_ID),
        "B" | "b" => Some(DEFAULT_BUS_B_ID),
        other => parse_digits(other.strip_prefix("bus")?),
    }
}

/// The step param a `ParamTarget::StepParam { param }` name addresses, as
/// the scheduler resolves process writes: `sync` / `delay` are not
/// writable per step and have no entry.
pub fn step_param_from_target_name(name: &str) -> Option<StepParam> {
    let normalized = name
        .trim_start_matches(':')
        .replace('_', "-")
        .to_ascii_lowercase();
    [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Speed,
        StepParam::AuxA,
        StepParam::AuxB,
        StepParam::Transpose,
        StepParam::Pan,
        StepParam::Chop,
        StepParam::Retrig,
        StepParam::RetrigRate,
    ]
    .into_iter()
    .find(|param| {
        param.short_label().eq_ignore_ascii_case(&normalized)
            || param
                .label()
                .replace(' ', "-")
                .eq_ignore_ascii_case(&normalized)
            || match param {
                StepParam::Duration => normalized == "duration",
                StepParam::Velocity => normalized == "velocity",
                StepParam::Speed => normalized == "speed",
                StepParam::AuxA => normalized == "aux-a",
                StepParam::AuxB => normalized == "aux-b",
                StepParam::Transpose => normalized == "transpose",
                StepParam::Pan => normalized == "pan",
                StepParam::Chop => normalized == "chop",
                StepParam::Retrig => normalized == "retrig",
                StepParam::RetrigRate => normalized == "retrig-rate" || normalized == "retrig_rate",
                StepParam::Sync | StepParam::Delay => false,
            }
    })
}

/// Strict unsigned decimal: non-empty, digits only (no sign, no spaces).
fn parse_digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A 1-based slot number in a label, returned 0-based.
fn parse_slot(text: &str, label: &str) -> Result<usize, String> {
    match parse_digits(text) {
        Some(n) if n >= 1 => Ok(n as usize - 1),
        _ => Err(format!(
            "bad slot number {text:?} in parameter name {label:?} (slots count from 1)"
        )),
    }
}

fn nonempty<'a>(text: &'a str, what: &str, label: &str) -> Result<&'a str, String> {
    if text.is_empty() {
        Err(format!("missing {what} in parameter name {label:?}"))
    } else {
        Ok(text)
    }
}

/// `NAME:PARAM` with both halves non-empty; PARAM keeps any further colons.
fn split_name_param<'a>(
    rest: &'a str,
    what: &str,
    label: &str,
) -> Result<(&'a str, &'a str), String> {
    let (name, param) = rest
        .split_once(':')
        .ok_or_else(|| format!("expected {what}:param in parameter name {label:?}"))?;
    Ok((
        nonempty(name, what, label)?,
        nonempty(param, "param", label)?,
    ))
}

impl ParamRef {
    /// Parse a macro-editor parameter label (see the table in
    /// docs/jaki-plock-spec.md §2). `process:…` inlets are rejected: they
    /// are not per-hit parameters.
    pub fn parse(label: &str) -> Result<ParamRef, String> {
        let text = label.trim();
        let (head, rest) = text
            .split_once(':')
            .ok_or_else(|| format!("unknown parameter name {label:?}"))?;
        match head {
            "instrument" | "instrument-param" => Ok(ParamRef::Instrument {
                param: nonempty(rest, "param", label)?.to_string(),
            }),
            "step-param" => {
                let name = nonempty(rest, "step param", label)?;
                step_param_from_target_name(name)
                    .map(|param| ParamRef::Step { param })
                    .ok_or_else(|| format!("unknown step param {name:?} in {label:?}"))
            }
            "effect-param" => {
                let (effect, param) = split_name_param(rest, "effect", label)?;
                Ok(ParamRef::Effect {
                    slot: None,
                    effect: effect.to_string(),
                    param: param.to_string(),
                })
            }
            "midi-fx-param" => {
                let (fx, param) = split_name_param(rest, "fx", label)?;
                Ok(ParamRef::MidiFx {
                    slot: None,
                    fx: fx.to_string(),
                    param: param.to_string(),
                })
            }
            "rack-macro" => {
                let index = rest
                    .strip_prefix("macro_")
                    .and_then(parse_digits)
                    .filter(|n| (1..=u8::MAX as u64).contains(n))
                    .ok_or_else(|| {
                        format!("expected rack-macro:macro_N (N from 1) in {label:?}")
                    })?;
                Ok(ParamRef::RackMacro {
                    macro_idx: index as usize - 1,
                })
            }
            "send" => bus_send_from_label(rest)
                .map(|bus| ParamRef::Send { bus })
                .ok_or_else(|| format!("unknown send bus {rest:?} in {label:?}")),
            "process" | "process-inlet" => Err(format!(
                "{label:?} is a process inlet, not a per-hit parameter"
            )),
            _ => {
                if let Some(slot) = head.strip_prefix("midi-fx") {
                    let slot = parse_slot(slot, label)?;
                    let (fx, param) = split_name_param(rest, "fx", label)?;
                    Ok(ParamRef::MidiFx {
                        slot: Some(slot),
                        fx: fx.to_string(),
                        param: param.to_string(),
                    })
                } else if let Some(slot) = head.strip_prefix("fx") {
                    let slot = parse_slot(slot, label)?;
                    let (effect, param) = split_name_param(rest, "effect", label)?;
                    Ok(ParamRef::Effect {
                        slot: Some(slot),
                        effect: effect.to_string(),
                        param: param.to_string(),
                    })
                } else if let Some(slot) = head.strip_prefix("rack") {
                    let slot = parse_slot(slot, label)?;
                    match rest.strip_prefix("instrument:") {
                        Some(param) => Ok(ParamRef::RackSlotInstrument {
                            slot,
                            param: nonempty(param, "param", label)?.to_string(),
                        }),
                        None => Ok(ParamRef::RackSlot {
                            slot,
                            param: nonempty(rest, "param", label)?.to_string(),
                        }),
                    }
                } else {
                    Err(format!("unknown parameter name {label:?}"))
                }
            }
        }
    }

    /// The unresolved reference a concrete target names: what
    /// `ParamRef::parse(&target.label())` yields. `None` for process inlets
    /// and for step-param names the scheduler does not write.
    pub fn from_target(target: &ParamTarget) -> Option<ParamRef> {
        Some(match target {
            ParamTarget::StepParam { param } => ParamRef::Step {
                param: step_param_from_target_name(param)?,
            },
            ParamTarget::InstrumentParam { param, .. } => ParamRef::Instrument {
                param: param.clone(),
            },
            ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            } => ParamRef::Effect {
                slot: Some(*slot),
                effect: effect.clone(),
                param: param.clone(),
            },
            ParamTarget::MidiFxParam { slot, fx, param } => ParamRef::MidiFx {
                slot: Some(*slot),
                fx: fx.clone(),
                param: param.clone(),
            },
            ParamTarget::ProcessInlet { .. } => return None,
            ParamTarget::RackSlotParam { slot, param } => ParamRef::RackSlot {
                slot: *slot,
                param: param.clone(),
            },
            ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
                ParamRef::RackSlotInstrument {
                    slot: *slot,
                    param: param.clone(),
                }
            }
            ParamTarget::RackMacroParam { macro_id } => ParamRef::RackMacro {
                macro_idx: *macro_id as usize,
            },
            ParamTarget::BusSend { bus } => ParamRef::Send { bus: *bus },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::ParamNodeId;
    use crate::process::ProcessInstanceId;

    /// One target per `ParamTarget` variant. Adding a variant fails to
    /// compile in `ParamTarget::label` / `ParamRef::from_target`, and this
    /// list is where its round-trip case goes.
    fn every_target() -> Vec<ParamTarget> {
        let mut targets: Vec<ParamTarget> = [
            "duration",
            "velocity",
            "speed",
            "aux-a",
            "aux-b",
            "transpose",
            "pan",
            "chop",
            "retrig",
            "retrig-rate",
        ]
        .into_iter()
        .map(|param| ParamTarget::StepParam {
            param: param.to_string(),
        })
        .collect();
        targets.extend([
            ParamTarget::InstrumentParam {
                param: "cutoff".to_string(),
                param_id: None,
            },
            ParamTarget::InstrumentParam {
                param: "filter_env".to_string(),
                param_id: Some(ParamNodeId {
                    logical_id: 7,
                    node_param_idx: 0,
                }),
            },
            ParamTarget::EffectParam {
                slot: 1,
                effect: "Filterbank".to_string(),
                param: "freq".to_string(),
                param_id: None,
            },
            ParamTarget::MidiFxParam {
                slot: 0,
                fx: "arp".to_string(),
                param: "rate".to_string(),
            },
            ParamTarget::RackSlotParam {
                slot: 2,
                param: "gain".to_string(),
            },
            ParamTarget::RackSlotInstrumentParam {
                slot: 2,
                param: "cutoff".to_string(),
                param_id: None,
            },
            ParamTarget::RackMacroParam { macro_id: 0 },
            ParamTarget::RackMacroParam { macro_id: 15 },
            ParamTarget::BusSend {
                bus: DEFAULT_BUS_A_ID,
            },
            ParamTarget::BusSend {
                bus: DEFAULT_BUS_B_ID,
            },
            ParamTarget::BusSend { bus: 42 },
        ]);
        targets
    }

    #[test]
    fn param_ref_parse_round_trips_every_target_label() {
        for target in every_target() {
            let label = target.label();
            let expected = ParamRef::from_target(&target)
                .unwrap_or_else(|| panic!("{target:?} should have a ParamRef"));
            assert_eq!(ParamRef::parse(&label), Ok(expected), "label {label:?}");
        }
    }

    #[test]
    fn param_ref_rejects_process_inlets() {
        let inlets = [
            ParamTarget::ProcessInlet {
                process: "lfo".to_string(),
                inlet: "rate".to_string(),
                instance_id: None,
            },
            ParamTarget::ProcessInlet {
                process: "lfo".to_string(),
                inlet: "rate".to_string(),
                instance_id: Some(ProcessInstanceId(3)),
            },
        ];
        for target in inlets {
            assert_eq!(ParamRef::from_target(&target), None);
            assert!(ParamRef::parse(&target.label()).is_err());
        }
        assert!(ParamRef::parse("process:lfo:rate").is_err());
    }

    #[test]
    fn param_ref_parses_slot_free_and_alias_spellings() {
        assert_eq!(
            ParamRef::parse("effect-param:filterbank:freq"),
            Ok(ParamRef::Effect {
                slot: None,
                effect: "filterbank".to_string(),
                param: "freq".to_string(),
            })
        );
        assert_eq!(
            ParamRef::parse("midi-fx-param:Arp:rate"),
            Ok(ParamRef::MidiFx {
                slot: None,
                fx: "Arp".to_string(),
                param: "rate".to_string(),
            })
        );
        assert_eq!(
            ParamRef::parse("instrument-param:cutoff"),
            ParamRef::parse("instrument:cutoff")
        );
        assert_eq!(
            ParamRef::parse("step-param:chop"),
            Ok(ParamRef::Step {
                param: StepParam::Chop
            })
        );
    }

    #[test]
    fn param_ref_keeps_effect_name_case() {
        assert_eq!(
            ParamRef::parse("fx2:FilterBank:freq"),
            Ok(ParamRef::Effect {
                slot: Some(1),
                effect: "FilterBank".to_string(),
                param: "freq".to_string(),
            })
        );
    }

    #[test]
    fn param_ref_rejects_malformed_labels() {
        for label in [
            "",
            "cutoff",
            "instrument:",
            "fx0:filterbank:freq",
            "fx:filterbank:freq",
            "fx2:filterbank",
            "fx2::freq",
            "midi-fx0:arp:rate",
            "rack0:gain",
            "rack1:",
            "rack-macro:macro_0",
            "rack-macro:1",
            "step-param:wobble",
            "step-param:sync",
            "send:C",
            "bogus:thing",
        ] {
            assert!(
                ParamRef::parse(label).is_err(),
                "{label:?} should not parse"
            );
        }
    }
}
