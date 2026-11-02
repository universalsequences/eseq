//! What a sample *is*, shared by sampler voices and audio-track region
//! players (docs/audio-tracks-spec.md §6): decoding to the engine's stereo
//! layout, the native-rate playback step, and interpolated reads.
//!
//! Anything a sampler voice *decides* (leading-silence trim, envelopes,
//! retrigger, loop modes) stays in `instruments::sampler`. A file must sound
//! identical through both players, so both read through these functions.

use std::path::Path;

/// A decoded file folded to interleaved stereo at its native rate.
pub struct StereoSample {
    pub sample_rate: u32,
    /// Channel count of the file before folding.
    pub source_channels: u16,
    /// Interleaved L/R frames.
    pub samples: Vec<f32>,
    pub warnings: Vec<String>,
}

impl StereoSample {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    /// Peak absolute amplitude over both channels.
    pub fn peak(&self) -> f32 {
        peak(&self.samples)
    }
}

/// Decode `path` and fold it to stereo. Callers resolve sample references
/// (`samples/<sha>.wav`) before calling.
pub fn decode_stereo(path: &Path) -> Result<StereoSample, String> {
    let decoded = eseqlisp::audio::sample::load_wav_file(path)?;
    let samples = fold_to_stereo(&decoded.samples, decoded.channels as usize);
    Ok(StereoSample {
        sample_rate: decoded.sample_rate,
        source_channels: decoded.channels,
        samples,
        warnings: decoded.warnings,
    })
}

/// Interleaved `channels`-channel samples as interleaved stereo: mono is
/// duplicated, stereo passes through, and wider files average their odd and
/// even channels into left and right.
pub fn fold_to_stereo(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        samples.iter().flat_map(|&sample| [sample, sample]).collect()
    } else if channels == 2 {
        samples.to_vec()
    } else {
        samples
            .chunks(channels)
            .flat_map(|ch| {
                let left = ch.iter().step_by(2).copied().sum::<f32>()
                    / ch.iter().step_by(2).count() as f32;
                let right = ch.iter().skip(1).step_by(2).copied().sum::<f32>()
                    / ch.iter().skip(1).step_by(2).count().max(1) as f32;
                [left, right]
            })
            .collect()
    }
}

pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

/// Source frames advanced per host frame. Sources stay at their native rate;
/// this ratio is the whole of the rate conversion.
pub fn playback_step(source_sample_rate: f32, host_sample_rate: f32, rate: f32) -> f32 {
    let source_sample_rate = source_sample_rate.max(1.0);
    let host_sample_rate = host_sample_rate.max(1.0);
    rate * source_sample_rate / host_sample_rate
}

/// [`playback_step`] at unit rate in `f64`, for players whose playhead must
/// stay frame-exact over minutes of audio.
pub fn playback_ratio(source_sample_rate: u32, host_sample_rate: f32) -> f64 {
    source_sample_rate.max(1) as f64 / (host_sample_rate as f64).max(1.0)
}

/// Linear-interpolated stereo read at `playhead` frames. Mono buffers read
/// the same sample on both sides; reads past the end are silence.
///
/// # Safety
/// `sample_data` must point at `sample_len * channel_count` readable floats.
pub unsafe fn read_interpolated(
    sample_data: *mut f32,
    sample_len: usize,
    channel_count: usize,
    playhead: f32,
) -> (f32, f32) {
    let idx = playhead as usize;
    let frac = playhead - idx as f32;
    let sample_index = idx * channel_count;
    let next_sample_index = (idx + 1) * channel_count;
    let s0_l = if idx < sample_len {
        *sample_data.add(sample_index)
    } else {
        0.0
    };
    let s1_l = if idx + 1 < sample_len {
        *sample_data.add(next_sample_index)
    } else {
        0.0
    };
    let s0_r = if channel_count > 1 && idx < sample_len {
        *sample_data.add(sample_index + 1)
    } else {
        s0_l
    };
    let s1_r = if channel_count > 1 && idx + 1 < sample_len {
        *sample_data.add(next_sample_index + 1)
    } else {
        s1_l
    };
    (s0_l + frac * (s1_l - s0_l), s0_r + frac * (s1_r - s0_r))
}

/// [`read_interpolated`] over an interleaved stereo slice at an `f64`
/// position, for sources long enough that an `f32` playhead would lose
/// whole frames (2^24 frames is about six minutes at 44.1 kHz).
pub fn read_stereo_at(stereo: &[f32], position: f64) -> (f32, f32) {
    if !(position >= 0.0) {
        return (0.0, 0.0);
    }
    let frames = stereo.len() / 2;
    let idx = position as usize;
    if idx >= frames {
        return (0.0, 0.0);
    }
    let frac = (position - idx as f64) as f32;
    let (s0_l, s0_r) = (stereo[idx * 2], stereo[idx * 2 + 1]);
    let (s1_l, s1_r) = if idx + 1 < frames {
        (stereo[idx * 2 + 2], stereo[idx * 2 + 3])
    } else {
        (0.0, 0.0)
    };
    (s0_l + frac * (s1_l - s0_l), s0_r + frac * (s1_r - s0_r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_duplicates_mono_and_averages_wide_files() {
        assert_eq!(fold_to_stereo(&[0.5, -0.25], 1), vec![0.5, 0.5, -0.25, -0.25]);
        assert_eq!(fold_to_stereo(&[0.1, 0.2], 2), vec![0.1, 0.2]);
        // Four channels: left = mean(ch0, ch2), right = mean(ch1, ch3).
        assert_eq!(fold_to_stereo(&[1.0, 0.0, 0.0, 1.0], 4), vec![0.5, 0.5]);
    }

    #[test]
    fn f64_read_matches_the_f32_read_inside_f32_range() {
        let mut stereo = vec![0.0, 1.0, 0.5, -0.5, 1.0, 0.25];
        for position in [0.0_f32, 0.25, 1.5, 1.999, 2.0, 2.5] {
            let expected =
                unsafe { read_interpolated(stereo.as_mut_ptr(), 3, 2, position) };
            assert_eq!(read_stereo_at(&stereo, position as f64), expected, "at {position}");
        }
    }

    #[test]
    fn f64_read_is_silent_outside_the_source() {
        let stereo = [0.5, 0.5];
        assert_eq!(read_stereo_at(&stereo, -0.5), (0.0, 0.0));
        assert_eq!(read_stereo_at(&stereo, 1.0), (0.0, 0.0));
        assert_eq!(read_stereo_at(&stereo, f64::NAN), (0.0, 0.0));
    }
}
