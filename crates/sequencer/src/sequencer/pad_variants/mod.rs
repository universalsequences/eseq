//! Drum-rack pad variants at runtime (docs/rack-pad-variants-spec.md §3.3, §4).
//!
//! The app publishes each member track's variants as param *names* and
//! values ([`RackPadVariantRuntime`]); every scheduler snapshot resolves them
//! against the track's current instrument into param indices
//! ([`PadVariantOverlays`]), so an instrument swap re-resolves without a
//! republish and the audio thread only does a lookup per hit.

use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::neural::ParamNodeId;

/// One variant of one member track as the app publishes it.
#[derive(Clone, Debug, PartialEq)]
pub struct RackPadVariantRuntime {
    pub track: usize,
    /// Never 0: the plain pad has no values to layer.
    pub variant: u32,
    /// Instrument param name -> value, in the param's own units.
    pub values: Vec<(String, f32)>,
}

/// One resolved value: the param index in the slot it targets, the param's
/// node identity at resolution (so a stale index is skipped, never applied
/// to a different param) and the value, already clamped to the param range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadVariantParam {
    pub param_idx: usize,
    pub identity: Option<ParamNodeId>,
    pub value: f32,
}

/// A variant resolved against one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadVariantOverlay {
    pub variant: u32,
    /// The track's own instrument slot (a plain instrument track).
    pub instrument: Vec<PadVariantParam>,
    /// A rack-type member (a kit pad): per rack slot index, the values that
    /// slot's instrument has a param for.
    pub rack_slots: Vec<(usize, Vec<PadVariantParam>)>,
}

impl PadVariantOverlay {
    pub fn rack_slot(&self, slot: usize) -> Option<&[PadVariantParam]> {
        self.rack_slots
            .iter()
            .find(|(idx, _)| *idx == slot)
            .map(|(_, params)| params.as_slice())
    }
}

/// Every track's resolved variants, indexed by track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadVariantOverlays {
    per_track: Vec<Vec<PadVariantOverlay>>,
}

impl PadVariantOverlays {
    /// The overlay a hit on `track` with `variant` plays; `None` for the
    /// plain pad and for an id the member has no pad for (spec §4.3: it
    /// plays as the plain pad).
    pub fn get(&self, track: usize, variant: u32) -> Option<&PadVariantOverlay> {
        if variant == 0 {
            return None;
        }
        self.per_track
            .get(track)?
            .iter()
            .find(|overlay| overlay.variant == variant)
    }

    pub fn is_empty(&self) -> bool {
        self.per_track.iter().all(Vec::is_empty)
    }

    fn push(&mut self, track: usize, overlay: PadVariantOverlay) {
        if self.per_track.len() <= track {
            self.per_track.resize_with(track + 1, Vec::new);
        }
        self.per_track[track].push(overlay);
    }
}

/// A step's variant id from its `StepParam::Variant` value.
pub fn variant_id(value: f32) -> u32 {
    if value.is_finite() && value >= 0.5 {
        value.round().min(crate::sequencer::MAX_PAD_VARIANT as f32) as u32
    } else {
        0
    }
}

/// Resolve `values` against one instrument slot: unknown names are skipped
/// (kept in the project, ignored here), values clamp to the param range.
pub fn resolve_slot_values(
    descriptor: &EffectDescriptor,
    slot: &EffectSlotSnapshot,
    values: &[(String, f32)],
) -> Vec<PadVariantParam> {
    let mut resolved = Vec::new();
    for (name, value) in values {
        if !value.is_finite() {
            continue;
        }
        let Ok(Some(param_idx)) = descriptor.resolve_param_index_by_tag_or_name(name) else {
            continue;
        };
        if param_idx >= slot.num_params as usize {
            continue;
        }
        let Some(desc) = descriptor.params.get(param_idx) else {
            continue;
        };
        let (lo, hi) = if desc.min <= desc.max {
            (desc.min, desc.max)
        } else {
            (desc.max, desc.min)
        };
        let identity = slot.node_param_idx(param_idx).and_then(|raw| {
            ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, raw)
        });
        resolved.push(PadVariantParam {
            param_idx,
            identity,
            value: value.clamp(lo, hi),
        });
    }
    resolved
}

/// Build the snapshot overlays from the published variants.
pub fn resolve_overlays(
    snapshot: &crate::sequencer::SequencerSnapshot,
    variants: &[RackPadVariantRuntime],
) -> PadVariantOverlays {
    let mut overlays = PadVariantOverlays::default();
    for variant in variants {
        if variant.variant == 0 {
            continue;
        }
        let Some(track) = snapshot.tracks.get(variant.track) else {
            continue;
        };
        let mut overlay = PadVariantOverlay {
            variant: variant.variant,
            ..PadVariantOverlay::default()
        };
        if let Some(rack) = track.rack_track.as_ref() {
            for (slot_idx, slot) in rack.slots.iter().enumerate() {
                let Some(desc) = snapshot.rack_slot_instrument_descriptor(slot) else {
                    continue;
                };
                let params = resolve_slot_values(desc, &slot.instrument_slot, &variant.values);
                if !params.is_empty() {
                    overlay.rack_slots.push((slot_idx, params));
                }
            }
        } else {
            overlay.instrument = resolve_slot_values(
                &track.instrument_descriptor,
                &track.instrument_slot,
                &variant.values,
            );
        }
        overlays.push(variant.track, overlay);
    }
    overlays
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_id_rounds_and_treats_non_positive_as_plain() {
        assert_eq!(variant_id(0.0), 0);
        assert_eq!(variant_id(-3.0), 0);
        assert_eq!(variant_id(f32::NAN), 0);
        assert_eq!(variant_id(2.0), 2);
        assert_eq!(variant_id(2.4), 2);
        assert_eq!(variant_id(1000.0), crate::sequencer::MAX_PAD_VARIANT);
    }

    #[test]
    fn overlays_skip_the_plain_pad_and_missing_variants() {
        let mut overlays = PadVariantOverlays::default();
        overlays.push(
            3,
            PadVariantOverlay {
                variant: 2,
                ..PadVariantOverlay::default()
            },
        );
        assert!(overlays.get(3, 2).is_some());
        assert!(overlays.get(3, 0).is_none(), "plain pad layers nothing");
        assert!(overlays.get(3, 7).is_none(), "missing id plays plain");
        assert!(overlays.get(9, 2).is_none());
    }
}
