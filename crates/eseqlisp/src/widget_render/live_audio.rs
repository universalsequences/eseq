use std::collections::HashMap;

use crate::vm::Value;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum LiveAudioSourceSelector {
    Master,
    Track {
        index: usize,
    },
    TrackEffect {
        index: usize,
        slot: usize,
    },
    Bus {
        id: Option<u64>,
        index: Option<usize>,
    },
    BusEffect {
        id: Option<u64>,
        index: Option<usize>,
        slot: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TapPoint {
    PreFx,
    PostFx,
}

impl LiveAudioSourceSelector {
    pub fn key_fragment(&self) -> String {
        match self {
            LiveAudioSourceSelector::Master => "master".to_string(),
            LiveAudioSourceSelector::Track { index } => format!("track:{index}"),
            LiveAudioSourceSelector::TrackEffect { index, slot } => {
                format!("track-effect:{index}:{slot}")
            }
            LiveAudioSourceSelector::Bus { id: Some(id), .. } => format!("bus-id:{id}"),
            LiveAudioSourceSelector::Bus {
                id: None,
                index: Some(index),
            } => format!("bus-index:{index}"),
            LiveAudioSourceSelector::Bus {
                id: None,
                index: None,
            } => "bus-invalid".to_string(),
            LiveAudioSourceSelector::BusEffect {
                id: Some(id), slot, ..
            } => {
                format!("bus-effect-id:{id}:{slot}")
            }
            LiveAudioSourceSelector::BusEffect {
                id: None,
                index: Some(index),
                slot,
            } => {
                format!("bus-effect-index:{index}:{slot}")
            }
            LiveAudioSourceSelector::BusEffect {
                id: None,
                index: None,
                slot,
            } => {
                format!("bus-effect-invalid:{slot}")
            }
        }
    }
}

impl TapPoint {
    pub fn key_fragment(self) -> &'static str {
        match self {
            TapPoint::PreFx => "pre-fx",
            TapPoint::PostFx => "post-fx",
        }
    }
}

pub fn source_from_props(props: &HashMap<String, Value>) -> LiveAudioSourceSelector {
    match props.get("source") {
        Some(Value::Keyword(source)) | Some(Value::String(source)) if source == "master" => {
            LiveAudioSourceSelector::Master
        }
        Some(Value::Map(map)) => source_from_map(map).unwrap_or(LiveAudioSourceSelector::Master),
        _ => LiveAudioSourceSelector::Master,
    }
}

pub fn tap_point_from_props(props: &HashMap<String, Value>) -> TapPoint {
    match prop_keyword(props, "tap-point").as_deref() {
        Some("pre-fx") | Some("pre") => TapPoint::PreFx,
        _ => TapPoint::PostFx,
    }
}

pub fn prop_keyword(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props.get(key).and_then(keyword_or_string)
}

pub fn keyword_or_string(value: &Value) -> Option<String> {
    match value {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn source_from_map(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
) -> Option<LiveAudioSourceSelector> {
    let kind = map
        .get("kind")
        .and_then(|value| keyword_or_string(&value.borrow()))?;
    match kind.as_str() {
        "master" => Some(LiveAudioSourceSelector::Master),
        "track" => {
            usize_from_map(map, "index").map(|index| LiveAudioSourceSelector::Track { index })
        }
        "track-effect" | "track-fx" => {
            let index = usize_from_map(map, "index")?;
            let slot = usize_from_map(map, "slot")
                .or_else(|| usize_from_map(map, "slot-idx"))
                .unwrap_or(0);
            Some(LiveAudioSourceSelector::TrackEffect { index, slot })
        }
        "bus" => {
            let id = u64_from_map(map, "id");
            let index = usize_from_map(map, "index");
            Some(LiveAudioSourceSelector::Bus { id, index })
        }
        "bus-effect" | "bus-fx" => {
            let id = u64_from_map(map, "id");
            let index = usize_from_map(map, "index");
            let slot = usize_from_map(map, "slot")
                .or_else(|| usize_from_map(map, "slot-idx"))
                .unwrap_or(0);
            Some(LiveAudioSourceSelector::BusEffect { id, index, slot })
        }
        _ => None,
    }
}

fn usize_from_map(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    key: &str,
) -> Option<usize> {
    match &*map.get(key)?.borrow() {
        Value::Number(value) if value.is_finite() && *value >= 0.0 => Some(value.round() as usize),
        Value::ReactiveRef { slot, .. } => {
            Some(crate::reactive::read_float_slot(slot).round().max(0.0) as usize)
        }
        _ => None,
    }
}

fn u64_from_map(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    key: &str,
) -> Option<u64> {
    match &*map.get(key)?.borrow() {
        Value::Number(value) if value.is_finite() && *value >= 0.0 => Some(value.round() as u64),
        Value::ReactiveRef { slot, .. } => {
            Some(crate::reactive::read_float_slot(slot).round().max(0.0) as u64)
        }
        _ => None,
    }
}
