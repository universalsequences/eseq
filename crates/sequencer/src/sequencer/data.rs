use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::audio::MAX_VOICES;

pub const MAX_TRACKS: usize = 64;
pub const MAX_RACK_SLOTS: usize = 16;
pub const DRUM_RACK_PAD_COUNT: usize = 16;
// Banks move by octave while the visible 4x4 grid spans sixteen chromatic pads.
pub const DRUM_RACK_PAD_BANK_STRIDE: i32 = 12;
pub const DRUM_RACK_TOTAL_PAD_NOTES: usize = 128;
pub const DRUM_RACK_FIRST_PAD_NOTE: i32 = 0;
pub const DRUM_RACK_LAST_PAD_NOTE: i32 =
    DRUM_RACK_FIRST_PAD_NOTE + DRUM_RACK_TOTAL_PAD_NOTES as i32 - 1;
pub const DRUM_RACK_LAST_PAD_BANK_START: i32 =
    DRUM_RACK_LAST_PAD_NOTE - DRUM_RACK_PAD_COUNT as i32 + 1;
pub const MAX_INSTRUMENT_ENGINES: usize = MAX_TRACKS * (MAX_RACK_SLOTS + 1);
pub const MAX_SAMPLER_POOLS: usize = MAX_TRACKS * (MAX_RACK_SLOTS + 1);
pub const MAX_STEPS: usize = 256;
pub const STEPS_PER_PAGE: usize = 16;
pub const NUM_PARAMS: usize = 10;
pub const DEFAULT_BPM: u32 = 120;
pub const TRACK_PATTERN_WORDS: usize = MAX_STEPS / 64;
pub const MIX_BUS_ID: u64 = 0;
pub const DEFAULT_BUS_A_ID: u64 = 1;
pub const DEFAULT_BUS_B_ID: u64 = 2;

pub fn rack_slot_pool_index(track_idx: usize, slot_idx: usize) -> Option<usize> {
    if track_idx >= MAX_TRACKS || slot_idx >= MAX_RACK_SLOTS {
        return None;
    }
    Some(MAX_TRACKS + track_idx * MAX_RACK_SLOTS + slot_idx)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BusId(pub u64);

impl BusId {
    pub const MIX: Self = Self(MIX_BUS_ID);
    pub const DEFAULT_A: Self = Self(DEFAULT_BUS_A_ID);
    pub const DEFAULT_B: Self = Self(DEFAULT_BUS_B_ID);
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TrackOutput {
    Mix,
    Bus(BusId),
    None,
}

impl Default for TrackOutput {
    fn default() -> Self {
        Self::Mix
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackSendSnapshot {
    pub destination: BusId,
    pub amount: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstrumentType {
    Sampler,
    Custom,
    Modulator,
    Rack,
}

impl InstrumentType {
    pub const COUNT: usize = 4;
    pub const ALL: [Self; Self::COUNT] = [Self::Sampler, Self::Custom, Self::Modulator, Self::Rack];
    pub const ADD_TRACK_TYPES: [Self; 3] = [Self::Sampler, Self::Custom, Self::Modulator];

    pub fn label(&self) -> &'static str {
        match self {
            InstrumentType::Sampler => "Sampler",
            InstrumentType::Custom => "Custom",
            InstrumentType::Modulator => "Modulator",
            InstrumentType::Rack => "Rack",
        }
    }

    pub fn runtime_flag(self) -> u32 {
        match self {
            InstrumentType::Sampler => 0,
            InstrumentType::Custom => 1,
            InstrumentType::Modulator => 2,
            InstrumentType::Rack => 3,
        }
    }

    pub fn from_runtime_flag(flag: u32) -> Self {
        match flag {
            1 => InstrumentType::Custom,
            2 => InstrumentType::Modulator,
            3 => InstrumentType::Rack,
            _ => InstrumentType::Sampler,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RackRouting {
    #[default]
    Broadcast,
    ByPitch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CustomInstrumentRunMode {
    #[default]
    Instrument,
    FreePatch,
}

impl CustomInstrumentRunMode {
    pub const COUNT: usize = 2;

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instrument => "instrument",
            Self::FreePatch => "free_patch",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "instrument" => Some(Self::Instrument),
            "free_patch" => Some(Self::FreePatch),
            _ => None,
        }
    }

    pub fn runtime_flag(self) -> u32 {
        match self {
            Self::Instrument => 0,
            Self::FreePatch => 1,
        }
    }

    pub fn from_runtime_flag(flag: u32) -> Self {
        match flag {
            1 => Self::FreePatch,
            _ => Self::Instrument,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModDestination {
    Track(usize),
    Bus(BusId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ModConnection {
    pub source_track: usize,
    pub destination: ModDestination,
    pub dest_input: usize,
}

pub const EXT_MOD_INPUT_COUNT: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Timebase {
    Whole = 0,
    Half = 1,
    Quarter = 2,
    Eighth = 3,
    Sixteenth = 4,
    ThirtySecond = 5,
    SixtyFourth = 6,
    HalfTriplet = 7,
    QuarterTriplet = 8,
    EighthTriplet = 9,
    SixteenthTriplet = 10,
    ThirtySecondTriplet = 11,
    SixtyFourthTriplet = 12,
    Polyrhythm = 13,
}

impl Timebase {
    pub const COUNT: usize = 14;

    pub const ALL: [Timebase; Self::COUNT] = [
        Timebase::Whole,
        Timebase::Half,
        Timebase::Quarter,
        Timebase::Eighth,
        Timebase::Sixteenth,
        Timebase::ThirtySecond,
        Timebase::SixtyFourth,
        Timebase::HalfTriplet,
        Timebase::QuarterTriplet,
        Timebase::EighthTriplet,
        Timebase::SixteenthTriplet,
        Timebase::ThirtySecondTriplet,
        Timebase::SixtyFourthTriplet,
        Timebase::Polyrhythm,
    ];

    pub const LABELS: [&'static str; Self::COUNT] = [
        "1", "2", "4", "8", "16", "32", "64", "2T", "4T", "8T", "16T", "32T", "64T", "Prh",
    ];

    pub fn from_index(i: u32) -> Self {
        Self::ALL
            .get(i as usize)
            .copied()
            .unwrap_or(Timebase::Sixteenth)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Timebase::Whole => "1",
            Timebase::Half => "2",
            Timebase::Quarter => "4",
            Timebase::Eighth => "8",
            Timebase::Sixteenth => "16",
            Timebase::ThirtySecond => "32",
            Timebase::SixtyFourth => "64",
            Timebase::HalfTriplet => "2T",
            Timebase::QuarterTriplet => "4T",
            Timebase::EighthTriplet => "8T",
            Timebase::SixteenthTriplet => "16T",
            Timebase::ThirtySecondTriplet => "32T",
            Timebase::SixtyFourthTriplet => "64T",
            Timebase::Polyrhythm => "Prh",
        }
    }

    pub fn step_beats(&self, num_steps: usize) -> f64 {
        match self {
            Timebase::Whole => 4.0,
            Timebase::Half => 2.0,
            Timebase::Quarter => 1.0,
            Timebase::Eighth => 0.5,
            Timebase::Sixteenth => 0.25,
            Timebase::ThirtySecond => 0.125,
            Timebase::SixtyFourth => 0.0625,
            Timebase::HalfTriplet => 4.0 / 3.0,
            Timebase::QuarterTriplet => 2.0 / 3.0,
            Timebase::EighthTriplet => 1.0 / 3.0,
            Timebase::SixteenthTriplet => 1.0 / 6.0,
            Timebase::ThirtySecondTriplet => 1.0 / 12.0,
            Timebase::SixtyFourthTriplet => 1.0 / 24.0,
            Timebase::Polyrhythm => 4.0 / num_steps.max(1) as f64,
        }
    }

    pub fn samples_per_step(&self, sample_rate: f64, bpm: f64, num_steps: usize) -> f64 {
        let samples_per_quarter = sample_rate * 60.0 / bpm;
        samples_per_quarter * self.step_beats(num_steps)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SwingResolution {
    Sixteenth = 0,
    Eighth = 1,
    Quarter = 2,
    Half = 3,
}

impl SwingResolution {
    pub const COUNT: usize = 4;
    pub const ALL: [Self; Self::COUNT] = [Self::Sixteenth, Self::Eighth, Self::Quarter, Self::Half];
    pub const LABELS: [&'static str; Self::COUNT] = ["1/16", "1/8", "1/4", "1/2"];

    pub fn from_index(i: u32) -> Self {
        Self::ALL
            .get(i as usize)
            .copied()
            .unwrap_or(Self::Sixteenth)
    }

    pub fn label(&self) -> &'static str {
        Self::LABELS[*self as usize]
    }

    pub fn step_beats(&self) -> f64 {
        match self {
            Self::Sixteenth => 0.25,
            Self::Eighth => 0.5,
            Self::Quarter => 1.0,
            Self::Half => 2.0,
        }
    }

    pub fn next(self) -> Self {
        Self::from_index((self as u32 + 1) % Self::COUNT as u32)
    }

    pub fn prev(self) -> Self {
        let idx = self as u32;
        if idx == 0 {
            Self::from_index(Self::COUNT as u32 - 1)
        } else {
            Self::from_index(idx - 1)
        }
    }
}

pub const SYNC_RESOLUTIONS: [(f64, &str); 8] = [
    (0.0, "Off"),
    (0.25, "1/16"),
    (0.5, "1/8"),
    (1.0, "1/4"),
    (2.0, "1/2 bar"),
    (4.0, "1 bar"),
    (8.0, "2 bars"),
    (16.0, "4 bars"),
];
pub const SYNC_COUNT: usize = SYNC_RESOLUTIONS.len();

pub fn sync_beats(val: f32) -> f64 {
    let idx = val.round() as usize;
    if idx > 0 && idx < SYNC_COUNT {
        SYNC_RESOLUTIONS[idx].0
    } else {
        0.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepParam {
    Duration = 0,
    Velocity = 1,
    Speed = 2,
    AuxA = 3,
    AuxB = 4,
    Transpose = 5,
    Pan = 6,
    Chop = 7,
    Sync = 8,
    Delay = 9,
}

impl StepParam {
    pub const ALL: [StepParam; NUM_PARAMS] = [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Speed,
        StepParam::AuxA,
        StepParam::AuxB,
        StepParam::Transpose,
        StepParam::Pan,
        StepParam::Chop,
        StepParam::Sync,
        StepParam::Delay,
    ];

    pub const VISIBLE: [StepParam; 7] = [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Delay,
        StepParam::AuxA,
        StepParam::Transpose,
        StepParam::Pan,
        StepParam::Sync,
    ];

    pub fn default_value(self) -> f32 {
        match self {
            StepParam::Duration => 1.0,
            StepParam::Velocity => 1.0,
            StepParam::Speed => 1.0,
            StepParam::AuxA => 0.0,
            StepParam::AuxB => 0.0,
            StepParam::Transpose => 0.0,
            StepParam::Pan => 0.0,
            StepParam::Chop => 1.0,
            StepParam::Sync => 0.0,
            StepParam::Delay => 0.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            StepParam::Duration => 0.0,
            StepParam::Velocity => 0.0,
            StepParam::Speed => 0.5,
            StepParam::AuxA => 0.0,
            StepParam::AuxB => 0.0,
            StepParam::Transpose => -48.0,
            StepParam::Pan => -1.0,
            StepParam::Chop => 1.0,
            StepParam::Sync => 0.0,
            StepParam::Delay => 0.0,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            StepParam::Duration => 32.0,
            StepParam::Velocity => 1.0,
            StepParam::Speed => 2.0,
            StepParam::AuxA => 16.0,
            StepParam::AuxB => 1.0,
            StepParam::Transpose => 48.0,
            StepParam::Pan => 1.0,
            StepParam::Chop => 8.0,
            StepParam::Sync => (SYNC_COUNT - 1) as f32,
            StepParam::Delay => 1.0,
        }
    }

    pub fn increment(self) -> f32 {
        match self {
            StepParam::Duration => 0.05,
            StepParam::Velocity => 0.05,
            StepParam::Speed => 0.05,
            StepParam::AuxA => 1.0,
            StepParam::AuxB => 0.05,
            StepParam::Transpose => 1.0,
            StepParam::Pan => 0.05,
            StepParam::Chop => 1.0,
            StepParam::Sync => 1.0,
            StepParam::Delay => 0.05,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StepParam::Duration => "Duration",
            StepParam::Velocity => "Velocity",
            StepParam::Speed => "Speed",
            StepParam::AuxA => "Aux A",
            StepParam::AuxB => "Aux B",
            StepParam::Transpose => "Transpose",
            StepParam::Pan => "Pan",
            StepParam::Chop => "Chop",
            StepParam::Sync => "Sync",
            StepParam::Delay => "Delay",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            StepParam::Duration => "dur",
            StepParam::Velocity => "vel",
            StepParam::Speed => "spd",
            StepParam::AuxA => "axA",
            StepParam::AuxB => "axB",
            StepParam::Transpose => "trn",
            StepParam::Pan => "pan",
            StepParam::Chop => "chp",
            StepParam::Sync => "syn",
            StepParam::Delay => "dly",
        }
    }

    pub fn normalize(self, val: f32) -> f32 {
        if self == StepParam::Duration {
            let val = val.clamp(self.slider_min(), self.slider_max());
            return if val <= 2.0 {
                val / 4.0
            } else {
                0.5 + 0.5 * ((val - 2.0) / 30.0).powf(0.25)
            };
        }
        let min = self.slider_min();
        let max = self.slider_max();
        if (max - min).abs() < f32::EPSILON {
            return 0.0;
        }
        ((val - min) / (max - min)).clamp(0.0, 1.0)
    }

    pub fn denormalize_slider(self, normalized: f32) -> f32 {
        let normalized = normalized.clamp(0.0, 1.0);
        if self == StepParam::Duration {
            return if normalized <= 0.5 {
                normalized * 4.0
            } else {
                2.0 + 30.0 * ((normalized - 0.5) * 2.0).powf(4.0)
            };
        }
        self.slider_min() + normalized * (self.slider_max() - self.slider_min())
    }

    pub fn slider_min(self) -> f32 {
        self.min()
    }

    pub fn slider_max(self) -> f32 {
        match self {
            StepParam::Duration => 32.0,
            _ => self.max(),
        }
    }

    pub fn format_value(self, val: f32) -> String {
        match self {
            StepParam::Transpose => format!("{:+.0}", val),
            StepParam::Pan => format!("{:+.2}", val),
            StepParam::Chop => format!("{:.0}", val),
            StepParam::Sync => {
                let idx = val.round() as usize;
                if idx < SYNC_COUNT {
                    SYNC_RESOLUTIONS[idx].1.to_string()
                } else {
                    "Off".to_string()
                }
            }
            StepParam::Delay => format!("{:.2}", val),
            _ => format!("{:.2}", val),
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn next(self) -> StepParam {
        let idx = (self.index() + 1) % NUM_PARAMS;
        StepParam::ALL[idx]
    }

    pub fn prev(self) -> StepParam {
        let idx = if self.index() == 0 {
            NUM_PARAMS - 1
        } else {
            self.index() - 1
        };
        StepParam::ALL[idx]
    }

    pub fn hotkey(self) -> char {
        match self {
            StepParam::Duration => 'd',
            StepParam::Velocity => 'v',
            StepParam::Speed => 's',
            StepParam::AuxA => 'a',
            StepParam::AuxB => 'b',
            StepParam::Transpose => 't',
            StepParam::Pan => 'p',
            StepParam::Chop => 'c',
            StepParam::Sync => 'y',
            StepParam::Delay => 'l',
        }
    }

    pub fn from_hotkey(c: char) -> Option<StepParam> {
        match c {
            'd' => Some(StepParam::Duration),
            'v' => Some(StepParam::Velocity),
            's' => Some(StepParam::Speed),
            'l' => Some(StepParam::Delay),
            'a' => Some(StepParam::AuxA),
            't' => Some(StepParam::Transpose),
            'p' => Some(StepParam::Pan),
            'y' => Some(StepParam::Sync),
            _ => None,
        }
    }

    pub fn tab_parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            StepParam::Duration => ("", "d", "ur"),
            StepParam::Velocity => ("", "v", "el"),
            StepParam::Speed => ("", "s", "pd"),
            StepParam::AuxA => ("", "a", "xA"),
            StepParam::AuxB => ("ax", "B", ""),
            StepParam::Transpose => ("", "t", "rn"),
            StepParam::Pan => ("", "p", "an"),
            StepParam::Chop => ("", "c", "hp"),
            StepParam::Sync => ("s", "y", "n"),
            StepParam::Delay => ("d", "l", "y"),
        }
    }
}

pub struct StepData {
    data: [AtomicU32; MAX_STEPS * NUM_PARAMS],
}

impl StepData {
    pub fn new() -> Self {
        let data: [AtomicU32; MAX_STEPS * NUM_PARAMS] = std::array::from_fn(|i| {
            let param_idx = i % NUM_PARAMS;
            let param = StepParam::ALL[param_idx];
            AtomicU32::new(param.default_value().to_bits())
        });
        Self { data }
    }

    pub fn get(&self, step: usize, param: StepParam) -> f32 {
        assert!(step < MAX_STEPS);
        let idx = step * NUM_PARAMS + param.index();
        f32::from_bits(self.data[idx].load(Ordering::Relaxed))
    }

    pub fn set(&self, step: usize, param: StepParam, val: f32) {
        assert!(step < MAX_STEPS);
        let clamped = val.clamp(param.min(), param.max());
        let idx = step * NUM_PARAMS + param.index();
        self.data[idx].store(clamped.to_bits(), Ordering::Relaxed);
    }

    /// Bulk read of every step's params in one flat pass (avoids the
    /// per-element call overhead of `get` in hot snapshot paths).
    pub fn load_rows(&self) -> Vec<[f32; NUM_PARAMS]> {
        let mut rows = Vec::with_capacity(MAX_STEPS);
        for step in 0..MAX_STEPS {
            let base = step * NUM_PARAMS;
            let mut params = [0.0f32; NUM_PARAMS];
            for (offset, slot) in params.iter_mut().enumerate() {
                *slot = f32::from_bits(self.data[base + offset].load(Ordering::Relaxed));
            }
            rows.push(params);
        }
        rows
    }

    /// Bulk write of step params with the same clamping as `set`. Missing
    /// rows are filled with parameter defaults.
    pub fn store_rows_clamped(&self, rows: &[[f32; NUM_PARAMS]]) {
        for step in 0..MAX_STEPS {
            let base = step * NUM_PARAMS;
            match rows.get(step) {
                Some(params) => {
                    for (offset, param) in StepParam::ALL.iter().enumerate() {
                        let clamped = params[offset].clamp(param.min(), param.max());
                        self.data[base + offset].store(clamped.to_bits(), Ordering::Relaxed);
                    }
                }
                None => {
                    for (offset, param) in StepParam::ALL.iter().enumerate() {
                        self.data[base + offset]
                            .store(param.default_value().to_bits(), Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

pub struct TrackPattern {
    bits: [AtomicU64; TRACK_PATTERN_WORDS],
}

impl TrackPattern {
    pub fn new() -> Self {
        Self {
            bits: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub fn toggle_step(&self, step: usize) {
        assert!(step < MAX_STEPS);
        let word = step / 64;
        let bit = step % 64;
        self.bits[word].fetch_xor(1u64 << bit, Ordering::Relaxed);
    }

    pub fn is_active(&self, step: usize) -> bool {
        assert!(step < MAX_STEPS);
        let word = step / 64;
        let bit = step % 64;
        (self.bits[word].load(Ordering::Relaxed) >> bit) & 1 == 1
    }

    pub fn load_bits(&self) -> [u64; TRACK_PATTERN_WORDS] {
        std::array::from_fn(|idx| self.bits[idx].load(Ordering::Relaxed))
    }

    pub fn store_bits(&self, bits: [u64; TRACK_PATTERN_WORDS]) {
        for (idx, word) in bits.into_iter().enumerate() {
            self.bits[idx].store(word, Ordering::Relaxed);
        }
    }

    pub fn clear_step(&self, step: usize) {
        assert!(step < MAX_STEPS);
        let word = step / 64;
        let bit = step % 64;
        self.bits[word].fetch_and(!(1u64 << bit), Ordering::Relaxed);
    }

    pub fn set_step_active(&self, step: usize, active: bool) {
        assert!(step < MAX_STEPS);
        let word = step / 64;
        let bit = step % 64;
        if active {
            self.bits[word].fetch_or(1u64 << bit, Ordering::Relaxed);
        } else {
            self.bits[word].fetch_and(!(1u64 << bit), Ordering::Relaxed);
        }
    }
}

pub struct TrackParams {
    pub gate: AtomicBool,
    pub attack_ms: AtomicU32,
    pub release_ms: AtomicU32,
    pub swing: AtomicU32,
    pub swing_resolution: AtomicU32,
    pub num_steps: AtomicU32,
    pub volume: AtomicU32,
    pub pan: AtomicU32,
    pub mute: AtomicBool,
    pub solo: AtomicBool,
    pub send: AtomicU32,
    pub output: Mutex<TrackOutput>,
    pub sends: Mutex<Vec<TrackSendSnapshot>>,
    pub polyphonic: AtomicBool,
    pub max_polyphony: AtomicU32,
    pub timebase: AtomicU32,
    pub accumulator_idx: AtomicU32,
    pub script_accumulator_name: Mutex<Option<String>>,
    pub midi_fx_chain: Mutex<Vec<String>>,
    pub midi_fx_position: AtomicU32,
    pub accum_limit: AtomicU32,
    pub accum_mode: AtomicU32,
    pub fts_scale: AtomicU32,
    pub mute_group: AtomicU32,
    pub global_transpose: AtomicBool,
}

impl TrackParams {
    pub fn new() -> Self {
        Self {
            gate: AtomicBool::new(true),
            attack_ms: AtomicU32::new(0.0_f32.to_bits()),
            release_ms: AtomicU32::new(0.0_f32.to_bits()),
            swing: AtomicU32::new(50.0_f32.to_bits()),
            swing_resolution: AtomicU32::new(SwingResolution::Sixteenth as u32),
            num_steps: AtomicU32::new(STEPS_PER_PAGE as u32),
            volume: AtomicU32::new(crate::mixer_volume::default_fader().to_bits()),
            pan: AtomicU32::new(0.0_f32.to_bits()),
            mute: AtomicBool::new(false),
            solo: AtomicBool::new(false),
            send: AtomicU32::new(0.0_f32.to_bits()),
            output: Mutex::new(TrackOutput::Mix),
            sends: Mutex::new(Vec::new()),
            polyphonic: AtomicBool::new(true),
            max_polyphony: AtomicU32::new(6),
            timebase: AtomicU32::new(Timebase::Sixteenth as u32),
            accumulator_idx: AtomicU32::new(0),
            script_accumulator_name: Mutex::new(None),
            midi_fx_chain: Mutex::new(Vec::new()),
            midi_fx_position: AtomicU32::new(MidiFxPosition::PostAccumulator as u32),
            accum_limit: AtomicU32::new(48.0_f32.to_bits()),
            accum_mode: AtomicU32::new(0),
            fts_scale: AtomicU32::new(0),
            mute_group: AtomicU32::new(0),
            global_transpose: AtomicBool::new(true),
        }
    }

    pub fn get_attack_ms(&self) -> f32 {
        f32::from_bits(self.attack_ms.load(Ordering::Relaxed))
    }
    pub fn set_attack_ms(&self, val: f32) {
        self.attack_ms
            .store(val.clamp(0.0, 500.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get_release_ms(&self) -> f32 {
        f32::from_bits(self.release_ms.load(Ordering::Relaxed))
    }
    pub fn set_release_ms(&self, val: f32) {
        self.release_ms
            .store(val.clamp(0.0, 2000.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get_swing(&self) -> f32 {
        f32::from_bits(self.swing.load(Ordering::Relaxed))
    }
    pub fn set_swing(&self, val: f32) {
        self.swing
            .store(val.clamp(50.0, 75.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get_swing_resolution(&self) -> SwingResolution {
        SwingResolution::from_index(self.swing_resolution.load(Ordering::Relaxed))
    }
    pub fn set_swing_resolution(&self, resolution: SwingResolution) {
        self.swing_resolution
            .store(resolution as u32, Ordering::Relaxed);
    }
    pub fn next_swing_resolution(&self) {
        self.set_swing_resolution(self.get_swing_resolution().next());
    }
    pub fn prev_swing_resolution(&self) {
        self.set_swing_resolution(self.get_swing_resolution().prev());
    }
    pub fn is_gate_on(&self) -> bool {
        self.gate.load(Ordering::Relaxed)
    }
    pub fn toggle_gate(&self) {
        self.gate.fetch_xor(true, Ordering::Relaxed);
    }
    pub fn get_num_steps(&self) -> usize {
        self.num_steps.load(Ordering::Relaxed) as usize
    }
    pub fn set_num_steps(&self, val: usize) {
        self.num_steps
            .store(val.clamp(1, MAX_STEPS) as u32, Ordering::Relaxed);
    }
    pub fn get_volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
    pub fn set_volume(&self, val: f32) {
        self.volume
            .store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get_pan(&self) -> f32 {
        f32::from_bits(self.pan.load(Ordering::Relaxed))
    }
    pub fn set_pan(&self, val: f32) {
        self.pan
            .store(val.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn is_muted(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }
    pub fn set_mute(&self, val: bool) {
        self.mute.store(val, Ordering::Relaxed);
    }
    pub fn toggle_mute(&self) -> bool {
        self.mute.fetch_xor(true, Ordering::Relaxed) ^ true
    }
    pub fn is_solo(&self) -> bool {
        self.solo.load(Ordering::Relaxed)
    }
    pub fn set_solo(&self, val: bool) {
        self.solo.store(val, Ordering::Relaxed);
    }
    pub fn toggle_solo(&self) -> bool {
        self.solo.fetch_xor(true, Ordering::Relaxed) ^ true
    }
    pub fn get_send(&self) -> f32 {
        f32::from_bits(self.send.load(Ordering::Relaxed))
    }
    pub fn set_send(&self, val: f32) {
        self.send
            .store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn output(&self) -> TrackOutput {
        self.output.lock().unwrap().clone()
    }
    pub fn set_output(&self, output: TrackOutput) {
        *self.output.lock().unwrap() = output;
    }
    pub fn sends(&self) -> Vec<TrackSendSnapshot> {
        self.sends.lock().unwrap().clone()
    }
    pub fn set_sends(&self, sends: Vec<TrackSendSnapshot>) {
        *self.sends.lock().unwrap() = sends
            .into_iter()
            .map(|mut send| {
                send.amount = send.amount.clamp(0.0, 1.0);
                send
            })
            .collect();
    }
    pub fn is_polyphonic(&self) -> bool {
        self.polyphonic.load(Ordering::Relaxed)
    }
    pub fn toggle_polyphonic(&self) {
        self.polyphonic.fetch_xor(true, Ordering::Relaxed);
    }
    pub fn get_max_polyphony(&self) -> usize {
        self.max_polyphony.load(Ordering::Relaxed) as usize
    }
    pub fn set_max_polyphony(&self, val: usize) {
        self.max_polyphony
            .store(val.clamp(1, MAX_VOICES) as u32, Ordering::Relaxed);
    }
    pub fn get_timebase(&self) -> Timebase {
        Timebase::from_index(self.timebase.load(Ordering::Relaxed))
    }
    pub fn set_timebase(&self, tb: Timebase) {
        self.timebase.store(tb as u32, Ordering::Relaxed);
    }
    pub fn next_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = (cur + 1) % Timebase::COUNT as u32;
        self.timebase.store(next, Ordering::Relaxed);
    }
    pub fn prev_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = if cur == 0 {
            Timebase::COUNT as u32 - 1
        } else {
            cur - 1
        };
        self.timebase.store(next, Ordering::Relaxed);
    }
    pub fn get_accumulator_idx(&self) -> usize {
        self.accumulator_idx.load(Ordering::Relaxed) as usize
    }
    pub fn set_accumulator_idx(&self, idx: usize) {
        self.accumulator_idx.store(idx as u32, Ordering::Relaxed);
    }
    pub fn script_accumulator_name(&self) -> Option<String> {
        self.script_accumulator_name.lock().unwrap().clone()
    }
    pub fn set_script_accumulator_name(&self, name: Option<String>) {
        *self.script_accumulator_name.lock().unwrap() = name;
    }
    pub fn midi_fx_chain(&self) -> Vec<String> {
        self.midi_fx_chain.lock().unwrap().clone()
    }
    pub fn set_midi_fx_chain(&self, chain: Vec<String>) {
        *self.midi_fx_chain.lock().unwrap() = chain;
    }
    pub fn get_midi_fx_position(&self) -> MidiFxPosition {
        MidiFxPosition::from_index(self.midi_fx_position.load(Ordering::Relaxed))
    }
    pub fn set_midi_fx_position(&self, position: MidiFxPosition) {
        self.midi_fx_position
            .store(position as u32, Ordering::Relaxed);
    }
    pub fn get_accum_limit(&self) -> f32 {
        f32::from_bits(self.accum_limit.load(Ordering::Relaxed))
    }
    pub fn set_accum_limit(&self, val: f32) {
        self.accum_limit
            .store(val.clamp(0.0, 127.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get_accum_mode(&self) -> u32 {
        self.accum_mode.load(Ordering::Relaxed)
    }
    pub fn set_accum_mode(&self, mode: u32) {
        self.accum_mode.store(mode, Ordering::Relaxed);
    }
    pub fn get_fts_scale(&self) -> usize {
        self.fts_scale.load(Ordering::Relaxed) as usize
    }
    pub fn set_fts_scale(&self, idx: usize) {
        self.fts_scale.store(idx as u32, Ordering::Relaxed);
    }
    pub fn get_mute_group(&self) -> u8 {
        self.mute_group.load(Ordering::Relaxed).min(8) as u8
    }
    pub fn set_mute_group(&self, group: u8) {
        self.mute_group
            .store(group.min(8) as u32, Ordering::Relaxed);
    }
    pub fn uses_global_transpose(&self) -> bool {
        self.global_transpose.load(Ordering::Relaxed)
    }
    pub fn set_global_transpose(&self, enabled: bool) {
        self.global_transpose.store(enabled, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiFxPosition {
    PreAccumulator = 0,
    PostAccumulator = 1,
}

impl MidiFxPosition {
    pub fn from_index(idx: u32) -> Self {
        match idx {
            0 => Self::PreAccumulator,
            _ => Self::PostAccumulator,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackParamsSnapshot {
    pub gate: bool,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub num_steps: usize,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub send: f32,
    pub output: TrackOutput,
    pub sends: Vec<TrackSendSnapshot>,
    pub polyphonic: bool,
    pub max_polyphony: usize,
    pub timebase: Timebase,
    pub accumulator_idx: usize,
    pub script_accumulator_name: Option<String>,
    pub midi_fx_chain: Vec<String>,
    pub midi_fx_position: MidiFxPosition,
    pub accum_limit: f32,
    pub accum_mode: u32,
    pub fts_scale: usize,
    pub mute_group: u8,
    pub global_transpose: bool,
}

impl Default for TrackParamsSnapshot {
    fn default() -> Self {
        Self {
            gate: true,
            attack_ms: 0.0,
            release_ms: 0.0,
            swing: 50.0,
            swing_resolution: SwingResolution::Sixteenth,
            num_steps: STEPS_PER_PAGE,
            volume: crate::mixer_volume::default_fader(),
            pan: 0.0,
            mute: false,
            solo: false,
            send: 0.0,
            output: TrackOutput::Mix,
            sends: Vec::new(),
            polyphonic: false,
            max_polyphony: 6,
            timebase: Timebase::Sixteenth,
            accumulator_idx: 0,
            script_accumulator_name: None,
            midi_fx_chain: Vec::new(),
            midi_fx_position: MidiFxPosition::PostAccumulator,
            accum_limit: 48.0,
            accum_mode: 0,
            fts_scale: 0,
            mute_group: 0,
            global_transpose: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrackSoundState {
    pub engine_id: Option<usize>,
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

pub struct ChordData {
    transposes: [AtomicU32; MAX_STEPS * MAX_VOICES],
    durations: [AtomicU32; MAX_STEPS * MAX_VOICES],
    delays: [AtomicU32; MAX_STEPS * MAX_VOICES],
    counts: [AtomicU32; MAX_STEPS],
}

impl ChordData {
    pub fn new() -> Self {
        Self {
            transposes: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            durations: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            delays: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            counts: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    pub fn count(&self, step: usize) -> usize {
        self.counts[step].load(Ordering::Relaxed) as usize
    }
    pub fn get(&self, step: usize, n: usize) -> f32 {
        f32::from_bits(self.transposes[step * MAX_VOICES + n].load(Ordering::Relaxed))
    }
    pub fn get_duration(&self, step: usize, n: usize) -> f32 {
        f32::from_bits(self.durations[step * MAX_VOICES + n].load(Ordering::Relaxed))
    }
    pub fn get_delay(&self, step: usize, n: usize) -> f32 {
        f32::from_bits(self.delays[step * MAX_VOICES + n].load(Ordering::Relaxed))
            .clamp(StepParam::Delay.min(), StepParam::Delay.max())
    }
    pub fn set_duration(&self, step: usize, n: usize, duration: f32) {
        if n < self.count(step).min(MAX_VOICES) {
            self.durations[step * MAX_VOICES + n]
                .store(duration.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }
    pub fn set_delay(&self, step: usize, n: usize, delay: f32) {
        if n < self.count(step).min(MAX_VOICES) {
            self.delays[step * MAX_VOICES + n].store(
                delay
                    .clamp(StepParam::Delay.min(), StepParam::Delay.max())
                    .to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    pub fn add_note(&self, step: usize, transpose: f32) -> bool {
        self.add_note_with_timing(step, transpose, 0.0, 0.0)
    }

    pub fn add_note_with_duration(&self, step: usize, transpose: f32, duration: f32) -> bool {
        self.add_note_with_timing(step, transpose, duration, 0.0)
    }

    pub fn add_note_with_timing(
        &self,
        step: usize,
        transpose: f32,
        duration: f32,
        delay: f32,
    ) -> bool {
        let c = self.counts[step].load(Ordering::Relaxed) as usize;
        if c >= MAX_VOICES {
            return false;
        }
        self.transposes[step * MAX_VOICES + c].store(transpose.to_bits(), Ordering::Relaxed);
        self.durations[step * MAX_VOICES + c].store(duration.max(0.0).to_bits(), Ordering::Relaxed);
        self.delays[step * MAX_VOICES + c].store(
            delay
                .clamp(StepParam::Delay.min(), StepParam::Delay.max())
                .to_bits(),
            Ordering::Relaxed,
        );
        self.counts[step].store((c + 1) as u32, Ordering::Relaxed);
        true
    }

    pub fn clear_step(&self, step: usize) {
        self.counts[step].store(0, Ordering::Relaxed);
    }

    /// Toggle a note: remove if present, add if absent. Returns true if added.
    pub fn toggle_note(&self, step: usize, transpose: f32) -> bool {
        let c = self.counts[step].load(Ordering::Relaxed) as usize;
        let bits = transpose.to_bits();
        for i in 0..c {
            if self.transposes[step * MAX_VOICES + i].load(Ordering::Relaxed) == bits {
                for j in i..(c - 1) {
                    let next = self.transposes[step * MAX_VOICES + j + 1].load(Ordering::Relaxed);
                    self.transposes[step * MAX_VOICES + j].store(next, Ordering::Relaxed);
                    let next_duration =
                        self.durations[step * MAX_VOICES + j + 1].load(Ordering::Relaxed);
                    self.durations[step * MAX_VOICES + j].store(next_duration, Ordering::Relaxed);
                    let next_delay = self.delays[step * MAX_VOICES + j + 1].load(Ordering::Relaxed);
                    self.delays[step * MAX_VOICES + j].store(next_delay, Ordering::Relaxed);
                }
                self.counts[step].store((c - 1) as u32, Ordering::Relaxed);
                return false;
            }
        }
        self.add_note(step, transpose)
    }

    pub fn copy_step(&self, src: usize, dst: usize) {
        let c = self.counts[src].load(Ordering::Relaxed);
        self.counts[dst].store(c, Ordering::Relaxed);
        for n in 0..(c as usize).min(MAX_VOICES) {
            let val = self.transposes[src * MAX_VOICES + n].load(Ordering::Relaxed);
            self.transposes[dst * MAX_VOICES + n].store(val, Ordering::Relaxed);
            let duration = self.durations[src * MAX_VOICES + n].load(Ordering::Relaxed);
            self.durations[dst * MAX_VOICES + n].store(duration, Ordering::Relaxed);
            let delay = self.delays[src * MAX_VOICES + n].load(Ordering::Relaxed);
            self.delays[dst * MAX_VOICES + n].store(delay, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChordSnapshot {
    pub steps: Vec<Vec<f32>>,
    pub durations: Vec<Vec<f32>>,
    pub delays: Vec<Vec<f32>>,
}

impl ChordSnapshot {
    pub fn capture(cd: &ChordData) -> Self {
        let mut steps = Vec::with_capacity(MAX_STEPS);
        let mut durations = Vec::with_capacity(MAX_STEPS);
        let mut delays = Vec::with_capacity(MAX_STEPS);
        for s in 0..MAX_STEPS {
            let c = cd.count(s);
            let mut notes = Vec::with_capacity(c);
            let mut note_durations = Vec::with_capacity(c);
            let mut note_delays = Vec::with_capacity(c);
            for n in 0..c {
                notes.push(cd.get(s, n));
                note_durations.push(cd.get_duration(s, n));
                note_delays.push(cd.get_delay(s, n));
            }
            steps.push(notes);
            durations.push(note_durations);
            delays.push(note_delays);
        }
        Self {
            steps,
            durations,
            delays,
        }
    }

    pub fn restore(&self, cd: &ChordData) {
        for s in 0..MAX_STEPS {
            let notes = &self.steps[s];
            cd.counts[s].store(notes.len() as u32, Ordering::Relaxed);
            for (n, &t) in notes.iter().enumerate() {
                if n < MAX_VOICES {
                    cd.transposes[s * MAX_VOICES + n].store(t.to_bits(), Ordering::Relaxed);
                    let duration = self
                        .durations
                        .get(s)
                        .and_then(|durations| durations.get(n))
                        .copied()
                        .unwrap_or(0.0);
                    cd.durations[s * MAX_VOICES + n]
                        .store(duration.max(0.0).to_bits(), Ordering::Relaxed);
                    let delay = self
                        .delays
                        .get(s)
                        .and_then(|delays| delays.get(n))
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(StepParam::Delay.min(), StepParam::Delay.max());
                    cd.delays[s * MAX_VOICES + n].store(delay.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    pub fn new_default() -> Self {
        Self {
            steps: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
            durations: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
            delays: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyboardTrigger {
    pub track: usize,
    pub transpose: f32,
    pub velocity: f32,
    pub note_off: bool,
}

pub struct Trigger {
    pub track: usize,
    pub step: usize,
    pub offset: usize,
    pub cycle_start_beats: f64,
}

pub struct TimebasePLockData {
    overrides: [AtomicU32; MAX_STEPS],
}

impl TimebasePLockData {
    pub fn new() -> Self {
        Self {
            overrides: std::array::from_fn(|_| AtomicU32::new(u32::MAX)),
        }
    }

    pub fn get(&self, step: usize) -> Option<Timebase> {
        let v = self.overrides[step].load(Ordering::Relaxed);
        if v == u32::MAX {
            None
        } else {
            Some(Timebase::from_index(v))
        }
    }

    pub fn set(&self, step: usize, tb: Timebase) {
        self.overrides[step].store(tb as u32, Ordering::Relaxed);
    }
    pub fn clear(&self, step: usize) {
        self.overrides[step].store(u32::MAX, Ordering::Relaxed);
    }
    pub fn has_plock(&self, step: usize) -> bool {
        self.overrides[step].load(Ordering::Relaxed) != u32::MAX
    }
    pub fn resolve(&self, step: usize, default: Timebase) -> Timebase {
        self.get(step).unwrap_or(default)
    }

    pub fn snapshot(&self) -> [Option<u32>; MAX_STEPS] {
        std::array::from_fn(|i| {
            let v = self.overrides[i].load(Ordering::Relaxed);
            if v == u32::MAX {
                None
            } else {
                Some(v)
            }
        })
    }

    pub fn restore(&self, snap: &[Option<u32>; MAX_STEPS]) {
        for (i, v) in snap.iter().enumerate() {
            self.overrides[i].store(v.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
}

pub struct SwingPLockData {
    overrides: [AtomicU32; MAX_STEPS],
}

impl SwingPLockData {
    pub fn new() -> Self {
        Self {
            overrides: std::array::from_fn(|_| AtomicU32::new(u32::MAX)),
        }
    }

    pub fn get(&self, step: usize) -> Option<f32> {
        let v = self.overrides[step].load(Ordering::Relaxed);
        if v == u32::MAX {
            None
        } else {
            Some(f32::from_bits(v))
        }
    }

    pub fn set(&self, step: usize, swing: f32) {
        self.overrides[step].store(swing.clamp(50.0, 75.0).to_bits(), Ordering::Relaxed);
    }

    pub fn clear(&self, step: usize) {
        self.overrides[step].store(u32::MAX, Ordering::Relaxed);
    }

    pub fn has_plock(&self, step: usize) -> bool {
        self.overrides[step].load(Ordering::Relaxed) != u32::MAX
    }

    pub fn resolve(&self, step: usize, default: f32) -> f32 {
        self.get(step).unwrap_or(default)
    }

    pub fn snapshot(&self) -> [Option<u32>; MAX_STEPS] {
        std::array::from_fn(|i| {
            let v = self.overrides[i].load(Ordering::Relaxed);
            if v == u32::MAX {
                None
            } else {
                Some(v)
            }
        })
    }

    pub fn restore(&self, snap: &[Option<u32>; MAX_STEPS]) {
        for (i, v) in snap.iter().enumerate() {
            self.overrides[i].store(v.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
}

pub struct SwingResolutionPLockData {
    overrides: [AtomicU32; MAX_STEPS],
}

impl SwingResolutionPLockData {
    pub fn new() -> Self {
        Self {
            overrides: std::array::from_fn(|_| AtomicU32::new(u32::MAX)),
        }
    }

    pub fn get(&self, step: usize) -> Option<SwingResolution> {
        let v = self.overrides[step].load(Ordering::Relaxed);
        if v == u32::MAX {
            None
        } else {
            Some(SwingResolution::from_index(v))
        }
    }

    pub fn set(&self, step: usize, resolution: SwingResolution) {
        self.overrides[step].store(resolution as u32, Ordering::Relaxed);
    }

    pub fn clear(&self, step: usize) {
        self.overrides[step].store(u32::MAX, Ordering::Relaxed);
    }

    pub fn has_plock(&self, step: usize) -> bool {
        self.overrides[step].load(Ordering::Relaxed) != u32::MAX
    }

    pub fn resolve(&self, step: usize, default: SwingResolution) -> SwingResolution {
        self.get(step).unwrap_or(default)
    }

    pub fn snapshot(&self) -> [Option<u32>; MAX_STEPS] {
        std::array::from_fn(|i| {
            let v = self.overrides[i].load(Ordering::Relaxed);
            if v == u32::MAX {
                None
            } else {
                Some(v)
            }
        })
    }

    pub fn restore(&self, snap: &[Option<u32>; MAX_STEPS]) {
        for (i, v) in snap.iter().enumerate() {
            self.overrides[i].store(v.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StepParam;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn duration_slider_uses_first_half_for_short_values() {
        assert_close(StepParam::Duration.normalize(0.0), 0.0);
        assert_close(StepParam::Duration.normalize(1.0), 0.25);
        assert_close(StepParam::Duration.normalize(2.0), 0.5);
        assert_close(StepParam::Duration.denormalize_slider(0.0), 0.0);
        assert_close(StepParam::Duration.denormalize_slider(0.25), 1.0);
        assert_close(StepParam::Duration.denormalize_slider(0.5), 2.0);
    }

    #[test]
    fn duration_slider_extends_second_half_to_thirty_two_steps() {
        assert_close(StepParam::Duration.normalize(32.0), 1.0);
        assert_close(StepParam::Duration.denormalize_slider(1.0), 32.0);
        assert_close(StepParam::Duration.denormalize_slider(0.75), 3.875);
    }
}
