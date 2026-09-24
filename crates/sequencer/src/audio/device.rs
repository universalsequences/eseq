/*!
CPAL output-device configuration selection.

Picks a sample rate and channel count from the device's supported format
ranges, preferring an independently discovered system-graph rate, then the
device default, and finally `FALLBACK_SAMPLE_RATE`/stereo. Pure logic over
`OutputFormatRange`s so it is unit-testable without real audio hardware.
*/

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OutputDeviceConfig {
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OutputFormatRange {
    pub(super) channels: u16,
    pub(super) min_sample_rate: u32,
    pub(super) max_sample_rate: u32,
    pub(super) supports_f32: bool,
}

impl OutputFormatRange {
    pub(super) fn supports_sample_rate(self, sample_rate: u32) -> bool {
        self.min_sample_rate <= sample_rate && sample_rate <= self.max_sample_rate
    }
}

pub(super) fn select_output_channels(
    sample_rate: u32,
    default_channels: u16,
    ranges: impl IntoIterator<Item = OutputFormatRange>,
) -> Option<u16> {
    ranges
        .into_iter()
        .filter(|range| range.supports_sample_rate(sample_rate))
        .filter(|range| range.supports_f32)
        .map(|range| range.channels)
        .min_by_key(|&channels| {
            let preference = if channels == default_channels {
                0
            } else if channels == 2 {
                1
            } else {
                2
            };
            (preference, channels)
        })
}

/// Channels to open on a CoreAudio output device reporting `device_channels`.
///
/// CPAL reports a macOS device's channel count as the sum over all of its
/// output streams, so a multi-stream interface (an Apollo's MON/LINE/HP/virtual
/// outputs, an aggregate device) would be opened as one wide interleaved
/// stream. The engine only renders a stereo mix, and the output unit accepts a
/// narrower client format than the device, routing it to the device's first
/// pair: outputs 1/2, which is what a DAW does by default.
pub(super) fn coreaudio_client_channels(device_channels: u16) -> u16 {
    device_channels.min(2)
}

pub(super) fn select_output_config(
    default_sample_rate: u32,
    default_channels: u16,
    ranges: impl IntoIterator<Item = OutputFormatRange>,
) -> Option<OutputDeviceConfig> {
    select_output_config_with_preferred_rate(None, default_sample_rate, default_channels, ranges)
}

pub(super) fn select_output_config_with_preferred_rate(
    preferred_sample_rate: Option<u32>,
    default_sample_rate: u32,
    default_channels: u16,
    ranges: impl IntoIterator<Item = OutputFormatRange>,
) -> Option<OutputDeviceConfig> {
    let ranges: Vec<OutputFormatRange> = ranges.into_iter().collect();
    let candidates = [preferred_sample_rate, Some(default_sample_rate), Some(FALLBACK_SAMPLE_RATE)];
    let mut previous = None;

    for sample_rate in candidates.into_iter().flatten() {
        if previous == Some(sample_rate) {
            continue;
        }
        previous = Some(sample_rate);
        if let Some(channels) =
            select_output_channels(sample_rate, default_channels, ranges.iter().copied())
        {
            return Some(OutputDeviceConfig {
                sample_rate,
                channels,
            });
        }
    }
    None
}

pub(super) fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}
