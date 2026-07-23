use super::*;

#[derive(Clone)]
pub struct RackSamplerBuildSpec {
    pub buffer_id: i32,
    pub sample_rate: u32,
    pub sample_name: String,
}

pub struct RackCustomBuildSpec<'a> {
    pub instrument_name: &'a str,
    pub engine_id: usize,
    pub manifest: &'a DGenManifest,
    pub lib: &'a LoadedDGenLib,
    pub run_mode: CustomInstrumentRunMode,
}

pub enum RackSlotInstrumentBuildSpec<'a> {
    Sampler(RackSamplerBuildSpec),
    Custom(RackCustomBuildSpec<'a>),
}

pub struct RackSlotBuildSpec<'a> {
    pub instrument: RackSlotInstrumentBuildSpec<'a>,
    pub instrument_base_note_offset: f32,
    pub pad_note: Option<i32>,
    pub choke_group: Option<u8>,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: Option<RackSlotParamPlocks>,
    pub instrument_slot: Option<EffectSlotSnapshot>,
    pub effect_slots: Option<Vec<EffectSlotSnapshot>>,
    pub effect_descriptors: Option<Vec<EffectDescriptor>>,
    pub custom_effect_names: Option<Vec<Option<String>>>,
    pub track_sound_state: Option<TrackSoundState>,
}
