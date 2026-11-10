//! Audio input (docs/audio-tracks-spec.md §12–§13): what an audio track
//! records from and monitors.
//!
//! ```text
//! cpal input callback ──► InputRing (per device, seconds of history)
//!                              │                    │
//!              the track's region player      take writer thread ──► WAV
//!              (monitoring, in the graph)     (recording)
//! ```
//!
//! - [`hub`]: one input stream per open device, owned by a worker thread.
//! - [`ring`]: the ring those streams fill, and the stream clocks that
//!   relate an input frame to the output sample playing when it was
//!   captured.
//! - [`take`]: the thread that writes a take from the ring to the audio
//!   store.
//!
//! Recording is placed by clock, not by when anything ran: the input and
//! output callbacks each stamp their buffers on the device clock, so a
//! recorded frame lands on the timeline under the sample the performer was
//! hearing when they played it, minus the converters' own latency
//! (the `arrangement-audio-record-offset-ms` knob, until it is measured).

use serde::{Deserialize, Serialize};

pub mod hub;
pub mod ring;
pub mod take;

pub use hub::{InputDeviceInfo, InputHub, InputState};
pub use ring::{output_epoch, stream_nanos, InputRing, StreamEpoch};
pub use take::{preview_rate, spawn_take, store_audio_bytes, FinishedTake, TakeHandle};

/// When an audio track's input is heard through its fx chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorMode {
    /// Never: the performer hears the source directly.
    Off,
    /// While the track is armed.
    #[default]
    Auto,
    /// Always.
    In,
}

impl MonitorMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::In => "in",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "in" => Some(Self::In),
            _ => None,
        }
    }

    pub fn hears(self, armed: bool) -> bool {
        match self {
            Self::Off => false,
            Self::Auto => armed,
            Self::In => true,
        }
    }
}

/// What an audio track records: a device by name and one channel of it, or
/// a pair. Saved with the project (spec §12.2); never the system default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrackInput {
    pub device: String,
    /// Zero-based device channel.
    pub left: u16,
    /// The right channel of a stereo input; a mono input has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<u16>,
    #[serde(default)]
    pub monitor: MonitorMode,
}

impl AudioTrackInput {
    /// "3" or "1/2": the channels as the picker names them.
    pub fn channel_label(&self) -> String {
        input_label("", self.left, self.right).trim_start().to_string()
    }

    /// The input as the picker names it.
    pub fn label(&self) -> String {
        input_label(&self.device, self.left, self.right)
    }
}

/// "Scarlett 3" or "Scarlett 1/2": how the picker names `device`'s channel
/// `left` (zero-based), or its pair with `right`. The picker matches its
/// options to a track's input by this text.
pub fn input_label(device: &str, left: u16, right: Option<u16>) -> String {
    match right {
        Some(right) => format!("{device} {}/{}", left + 1, right + 1),
        None => format!("{device} {}", left + 1),
    }
}

/// A track's input as a project file stores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAudioInput {
    pub track: crate::sequencer::TrackId,
    #[serde(flatten)]
    pub input: AudioTrackInput,
}

/// A built-in microphone: monitoring it through the speakers beside it
/// feeds back, so it starts unmonitored (spec §12.5).
pub fn feeds_back_by_default(device: &str) -> bool {
    let name = device.to_ascii_lowercase();
    name.contains("microphone") || name.contains("built-in")
}

#[cfg(test)]
mod tests;
