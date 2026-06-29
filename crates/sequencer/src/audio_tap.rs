use std::ffi::c_void;
use std::os::raw::c_int;

use crate::audiograph::NodeVTable;

pub const TAP_CHANNELS: usize = 2;
pub const STATE_WRITE_HEAD: usize = 0;
pub const STATE_RING_FRAMES: usize = 1;
pub const STATE_SAMPLE_RATE: usize = 2;
pub const STATE_CHANNELS: usize = 3;
pub const STATE_DATA_START: usize = 4;

pub const MIN_TAP_RING_FRAMES: usize = 2048;
pub const MAX_TAP_RING_FRAMES: usize = 32768;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapMetadata {
    pub write_head: usize,
    pub ring_frames: usize,
    pub sample_rate: f32,
    pub channels: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrogramFrameData {
    pub revision: u64,
    pub bins: u32,
    pub time_slices: u32,
    pub write_head: u32,
    pub sample_rate: f32,
    pub waterfall: Vec<f32>,
    pub smoothed: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct SpectrogramProcessor {
    fft_size: usize,
    time_slices: usize,
    min_db: f32,
    max_db: f32,
    smoothing: f32,
    write_head: usize,
    revision: u64,
    last_tap_write_head: Option<usize>,
    waterfall: Vec<f32>,
    smoothed: Vec<f32>,
}

impl SpectrogramProcessor {
    pub fn new(
        fft_size: usize,
        time_slices: usize,
        min_db: f32,
        max_db: f32,
        smoothing: f32,
    ) -> Self {
        assert!(fft_size.is_power_of_two());
        assert!(fft_size >= 2);
        assert!(time_slices > 0);
        let bins = fft_size / 2;
        Self {
            fft_size,
            time_slices,
            min_db,
            max_db: max_db.max(min_db + 1.0),
            smoothing: smoothing.clamp(0.0, 0.98),
            write_head: 0,
            revision: 0,
            last_tap_write_head: None,
            waterfall: vec![0.0; bins * time_slices],
            smoothed: vec![0.0; bins],
        }
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    pub fn time_slices(&self) -> usize {
        self.time_slices
    }

    pub fn bins(&self) -> usize {
        self.fft_size / 2
    }

    pub fn update_from_tap_state(&mut self, state: &[f32]) -> Option<SpectrogramFrameData> {
        let metadata = tap_metadata(state)?;
        if self.last_tap_write_head == Some(metadata.write_head) {
            return None;
        }
        let samples = read_latest_mono(state, self.fft_size)?;
        let spectrum = compute_normalized_spectrum(&samples, self.min_db, self.max_db);
        let bins = self.bins();
        let row_start = self.write_head * bins;
        self.waterfall[row_start..row_start + bins].copy_from_slice(&spectrum);

        let smooth = self.smoothing;
        if self.revision == 0 {
            self.smoothed.copy_from_slice(&spectrum);
        } else {
            for (smoothed, value) in self.smoothed.iter_mut().zip(spectrum.iter()) {
                *smoothed = *smoothed * smooth + *value * (1.0 - smooth);
            }
        }

        self.write_head = (self.write_head + 1) % self.time_slices;
        self.revision = self.revision.wrapping_add(1);
        self.last_tap_write_head = Some(metadata.write_head);

        Some(SpectrogramFrameData {
            revision: self.revision,
            bins: bins as u32,
            time_slices: self.time_slices as u32,
            write_head: self.write_head as u32,
            sample_rate: metadata.sample_rate,
            waterfall: self.waterfall.clone(),
            smoothed: self.smoothed.clone(),
        })
    }
}

pub fn normalize_ring_frames(requested: usize) -> usize {
    requested
        .max(MIN_TAP_RING_FRAMES)
        .next_power_of_two()
        .clamp(MIN_TAP_RING_FRAMES, MAX_TAP_RING_FRAMES)
}

pub fn state_len_floats(ring_frames: usize) -> usize {
    STATE_DATA_START + normalize_ring_frames(ring_frames) * TAP_CHANNELS
}

pub fn state_size_bytes(ring_frames: usize) -> usize {
    state_len_floats(ring_frames) * std::mem::size_of::<f32>()
}

pub fn initial_state(ring_frames: usize) -> Vec<f32> {
    let ring_frames = normalize_ring_frames(ring_frames);
    let mut state = vec![0.0; STATE_DATA_START + ring_frames * TAP_CHANNELS];
    state[STATE_WRITE_HEAD] = 0.0;
    state[STATE_RING_FRAMES] = ring_frames as f32;
    state[STATE_SAMPLE_RATE] = 0.0;
    state[STATE_CHANNELS] = TAP_CHANNELS as f32;
    state
}

pub fn initialize_state_buffer(state: &mut [f32], ring_frames: usize, sample_rate: f32) -> bool {
    let ring_frames = normalize_ring_frames(ring_frames);
    let required_len = STATE_DATA_START + ring_frames * TAP_CHANNELS;
    if state.len() < required_len {
        return false;
    }
    state[STATE_WRITE_HEAD] = 0.0;
    state[STATE_RING_FRAMES] = ring_frames as f32;
    state[STATE_SAMPLE_RATE] = sample_rate.max(1.0);
    state[STATE_CHANNELS] = TAP_CHANNELS as f32;
    state[STATE_DATA_START..required_len].fill(0.0);
    true
}

pub fn tap_metadata(state: &[f32]) -> Option<TapMetadata> {
    if state.len() < STATE_DATA_START {
        return None;
    }
    let ring_frames = state[STATE_RING_FRAMES].round() as usize;
    let channels = state[STATE_CHANNELS].round() as usize;
    if ring_frames == 0 || channels != TAP_CHANNELS {
        return None;
    }
    let required_len = STATE_DATA_START + ring_frames * TAP_CHANNELS;
    if state.len() < required_len {
        return None;
    }
    let write_head = (state[STATE_WRITE_HEAD].round() as usize) % ring_frames;
    let sample_rate = state[STATE_SAMPLE_RATE].max(1.0);
    Some(TapMetadata {
        write_head,
        ring_frames,
        sample_rate,
        channels,
    })
}

pub fn read_latest_mono(state: &[f32], frame_count: usize) -> Option<Vec<f32>> {
    let metadata = tap_metadata(state)?;
    let frames = frame_count.min(metadata.ring_frames);
    let mut samples = Vec::with_capacity(frames);
    for offset in 0..frames {
        let ring_index =
            (metadata.write_head + metadata.ring_frames - frames + offset) % metadata.ring_frames;
        let data_index = STATE_DATA_START + ring_index * TAP_CHANNELS;
        samples.push((state[data_index] + state[data_index + 1]) * 0.5);
    }
    Some(samples)
}

pub fn compute_normalized_spectrum(samples: &[f32], min_db: f32, max_db: f32) -> Vec<f32> {
    assert!(samples.len().is_power_of_two());
    let n = samples.len();
    let mut real = vec![0.0f32; n];
    let mut imag = vec![0.0f32; n];
    let denom = (n - 1).max(1) as f32;
    for (i, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / denom).cos();
        real[i] = *sample * window;
    }
    fft_radix2(&mut real, &mut imag);

    let db_range = (max_db - min_db).max(1.0);
    let bins = n / 2;
    let scale = 2.0 / n as f32;
    let mut spectrum = Vec::with_capacity(bins);
    for bin in 0..bins {
        let mag = (real[bin] * real[bin] + imag[bin] * imag[bin]).sqrt() * scale;
        let db = 20.0 * mag.max(1.0e-10).log10();
        spectrum.push(((db - min_db) / db_range).clamp(0.0, 1.0));
    }
    spectrum
}

fn fft_radix2(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    debug_assert!(n == imag.len());
    debug_assert!(n.is_power_of_two());

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let wlen_real = angle.cos();
        let wlen_imag = angle.sin();
        for start in (0..n).step_by(len) {
            let mut w_real = 1.0f32;
            let mut w_imag = 0.0f32;
            for i in 0..(len / 2) {
                let even = start + i;
                let odd = even + len / 2;
                let odd_real = real[odd] * w_real - imag[odd] * w_imag;
                let odd_imag = real[odd] * w_imag + imag[odd] * w_real;
                real[odd] = real[even] - odd_real;
                imag[odd] = imag[even] - odd_imag;
                real[even] += odd_real;
                imag[even] += odd_imag;

                let next_real = w_real * wlen_real - w_imag * wlen_imag;
                let next_imag = w_real * wlen_imag + w_imag * wlen_real;
                w_real = next_real;
                w_imag = next_imag;
            }
        }
        len <<= 1;
    }
}

unsafe extern "C" fn audio_tap_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    let mut ring_frames = MIN_TAP_RING_FRAMES;
    if !initial_state.is_null() {
        let initial = initial_state as *const f32;
        let requested = (*initial.add(STATE_RING_FRAMES)).round() as usize;
        ring_frames = normalize_ring_frames(requested);
    }
    let len = STATE_DATA_START + ring_frames * TAP_CHANNELS;
    let state = std::slice::from_raw_parts_mut(s, len);
    initialize_state_buffer(state, ring_frames, sample_rate.max(1) as f32);
}

unsafe extern "C" fn audio_tap_process(
    inp: *const *mut f32,
    _out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() || nframes <= 0 {
        return;
    }
    let s = state as *mut f32;
    let ring_frames = (*s.add(STATE_RING_FRAMES)).round().max(1.0) as usize;
    let mut write_head = (*s.add(STATE_WRITE_HEAD)).round().max(0.0) as usize % ring_frames;
    let input_l = if inp.is_null() {
        std::ptr::null_mut()
    } else {
        *inp.add(0)
    };
    let input_r = if inp.is_null() {
        std::ptr::null_mut()
    } else {
        *inp.add(1)
    };

    for i in 0..nframes as usize {
        let left = if input_l.is_null() {
            0.0
        } else {
            *input_l.add(i)
        };
        let right = if input_r.is_null() {
            left
        } else {
            *input_r.add(i)
        };
        let data_index = STATE_DATA_START + write_head * TAP_CHANNELS;
        *s.add(data_index) = left;
        *s.add(data_index + 1) = right;
        write_head = (write_head + 1) % ring_frames;
    }
    *s.add(STATE_WRITE_HEAD) = write_head as f32;
}

pub fn audio_tap_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(audio_tap_process),
        init: Some(audio_tap_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_tap(state: &mut [f32], left: &[f32], right: &[f32]) {
        let mut left = left.to_vec();
        let mut right = right.to_vec();
        let inputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            audio_tap_process(
                inputs.as_ptr(),
                std::ptr::null(),
                left.len() as c_int,
                state.as_mut_ptr() as *mut c_void,
                std::ptr::null_mut(),
            );
        }
    }

    #[test]
    fn tap_state_layout_is_bounded_and_float_addressed() {
        assert_eq!(normalize_ring_frames(1), MIN_TAP_RING_FRAMES);
        assert_eq!(
            normalize_ring_frames(MAX_TAP_RING_FRAMES * 2),
            MAX_TAP_RING_FRAMES
        );
        let state = initial_state(4096);
        assert_eq!(state[STATE_RING_FRAMES], 4096.0);
        assert_eq!(state[STATE_CHANNELS], 2.0);
        assert_eq!(state.len(), STATE_DATA_START + 4096 * 2);
        assert_eq!(
            state_size_bytes(4096),
            state.len() * std::mem::size_of::<f32>()
        );
    }

    #[test]
    fn tap_init_sets_sample_rate_and_clears_ring() {
        let initial = initial_state(4096);
        let mut state = vec![9.0; initial.len()];
        assert!(initialize_state_buffer(&mut state, 4096, 44_100.0));
        assert_eq!(state[STATE_SAMPLE_RATE], 44_100.0);
        assert_eq!(state[STATE_RING_FRAMES], 4096.0);
        assert!(state[STATE_DATA_START..].iter().all(|value| *value == 0.0));
    }

    #[test]
    fn tap_captures_stereo_input_and_wraps_ring() {
        let mut state = initial_state(MIN_TAP_RING_FRAMES);
        state[STATE_RING_FRAMES] = 4.0;
        run_tap(&mut state, &[1.0, 2.0, 3.0], &[10.0, 20.0, 30.0]);
        assert_eq!(tap_metadata(&state).unwrap().write_head, 3);
        run_tap(&mut state, &[4.0, 5.0], &[40.0, 50.0]);
        assert_eq!(tap_metadata(&state).unwrap().write_head, 1);
        assert_eq!(
            read_latest_mono(&state, 4).unwrap(),
            vec![11.0, 16.5, 22.0, 27.5]
        );
    }

    #[test]
    fn normalized_spectrum_detects_sine_bin() {
        let fft_size = 1024;
        let bin = 8usize;
        let samples = (0..fft_size)
            .map(|i| (2.0 * std::f32::consts::PI * bin as f32 * i as f32 / fft_size as f32).sin())
            .collect::<Vec<_>>();
        let spectrum = compute_normalized_spectrum(&samples, -90.0, 0.0);
        let peak_bin = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        assert!((peak_bin as isize - bin as isize).abs() <= 1);
        assert!(spectrum[peak_bin] > 0.8);
    }

    #[test]
    fn spectrogram_processor_smooths_and_advances_waterfall() {
        let mut state = initial_state(2048);
        state[STATE_RING_FRAMES] = 2048.0;
        state[STATE_SAMPLE_RATE] = 48_000.0;
        let mut processor = SpectrogramProcessor::new(1024, 4, -90.0, 0.0, 0.5);

        let samples = (0..2048)
            .map(|i| (2.0 * std::f32::consts::PI * 8.0 * i as f32 / 1024.0).sin())
            .collect::<Vec<_>>();
        let right = samples.clone();
        run_tap(&mut state, &samples, &right);
        let first = processor.update_from_tap_state(&state).unwrap();
        assert_eq!(first.write_head, 1);
        assert_eq!(first.bins, 512);
        assert_eq!(first.time_slices, 4);

        let silence = vec![0.0; 128];
        run_tap(&mut state, &silence, &silence);
        let second = processor.update_from_tap_state(&state).unwrap();
        assert_eq!(second.write_head, 2);
        assert!(second.revision > first.revision);
        assert!(second
            .smoothed
            .iter()
            .zip(first.smoothed.iter())
            .any(|(a, b)| a < b));
    }
}
