use crate::lisp_host::{
    render_effect_source_for_test, render_instrument_source_for_test,
    render_loaded_effect_for_test, render_loaded_instrument_for_test, CompileResult,
    EffectRenderOptions, EffectRenderReport, InstrumentRenderOptions, InstrumentRenderReport,
};

use super::store::AuditionResult;

const SILENT_PEAK: f32 = 0.001;
const CLIP_PEAK: f32 = 1.0;
const PASSTHROUGH_DIFF_RMS: f32 = 0.0005;

pub fn audition_instrument_source(
    source: &str,
    sample_rate: u32,
) -> Result<AuditionResult, String> {
    let options = default_options(sample_rate);
    let report = render_instrument_source_for_test(source, None, &options)?;
    Ok(classify_report(report, sample_rate))
}

pub fn audition_loaded_instrument(
    result: &CompileResult,
    sample_rate: u32,
) -> Result<AuditionResult, String> {
    let options = default_options(sample_rate);
    let report = render_loaded_instrument_for_test(&result.manifest, &result.lib, &options)?;
    Ok(classify_report(report, sample_rate))
}

pub fn audition_effect_source(source: &str, sample_rate: u32) -> Result<AuditionResult, String> {
    let options = default_effect_options(sample_rate);
    let report = render_effect_source_for_test(source, &options)?;
    Ok(classify_effect_report(report, sample_rate))
}

pub fn audition_loaded_effect(
    result: &CompileResult,
    sample_rate: u32,
) -> Result<AuditionResult, String> {
    let options = default_effect_options(sample_rate);
    let report = render_loaded_effect_for_test(&result.manifest, &result.lib, &options)?;
    Ok(classify_effect_report(report, sample_rate))
}

fn default_options(sample_rate: u32) -> InstrumentRenderOptions {
    InstrumentRenderOptions {
        sample_rate,
        block_size: 128,
        frames: 26_460,
        midi_note: 69.0,
        velocity: 1.0,
        gate_frames: 22_050,
        voice_index: 0,
        param_overrides: Vec::new(),
        param_events: Vec::new(),
        input_overrides: Vec::new(),
    }
}

fn default_effect_options(sample_rate: u32) -> EffectRenderOptions {
    EffectRenderOptions {
        sample_rate,
        block_size: 128,
        frames: 26_460,
        param_overrides: Vec::new(),
        input_overrides: Vec::new(),
    }
}

fn classify_report(report: InstrumentRenderReport, sample_rate: u32) -> AuditionResult {
    AuditionResult {
        ran: true,
        peak_db: amp_to_db(report.peak),
        rms_db: amp_to_db(report.rms),
        clipped: report.peak > CLIP_PEAK,
        duration_ms: ((report.frames as f64 / sample_rate.max(1) as f64) * 1000.0).round() as u32,
        silent: report.peak < SILENT_PEAK,
        differs_from_input: None,
        diff_rms_db: None,
    }
}

fn classify_effect_report(report: EffectRenderReport, sample_rate: u32) -> AuditionResult {
    AuditionResult {
        ran: true,
        peak_db: amp_to_db(report.peak),
        rms_db: amp_to_db(report.rms),
        clipped: report.peak > CLIP_PEAK,
        duration_ms: ((report.frames as f64 / sample_rate.max(1) as f64) * 1000.0).round() as u32,
        silent: report.peak < SILENT_PEAK,
        differs_from_input: Some(report.diff_rms >= PASSTHROUGH_DIFF_RMS),
        diff_rms_db: Some(amp_to_db(report.diff_rms)),
    }
}

pub fn audition_feedback(result: &AuditionResult) -> String {
    if result.silent {
        "audition: SILENT. Likely no signal; check envelope, gain stage, or signal routing."
            .to_string()
    } else if result.clipped {
        format!(
            "audition: CLIPPED (peak {:.1} dB). Reduce gain.",
            result.peak_db
        )
    } else if result.differs_from_input == Some(false) {
        format!(
            "audition: PASSTHROUGH (diff rms {:.1} dB). The effect output is too close to the input; make the effect audibly change the signal.",
            result.diff_rms_db.unwrap_or(f32::NEG_INFINITY)
        )
    } else {
        format!(
            "audition: peak {:.1} dB, rms {:.1} dB, ran {}ms",
            result.peak_db, result.rms_db, result.duration_ms
        )
    }
}

fn amp_to_db(amp: f32) -> f32 {
    if amp <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::{amp_to_db, audition_feedback};
    use crate::agent::store::AuditionResult;

    #[test]
    fn amp_db_handles_zero() {
        assert!(amp_to_db(0.0).is_infinite());
        assert!((amp_to_db(1.0) - 0.0).abs() < 0.0001);
    }

    #[test]
    fn feedback_classifies_silent() {
        let result = AuditionResult {
            ran: true,
            peak_db: f32::NEG_INFINITY,
            rms_db: f32::NEG_INFINITY,
            clipped: false,
            duration_ms: 600,
            silent: true,
            differs_from_input: None,
            diff_rms_db: None,
        };
        assert!(audition_feedback(&result).contains("SILENT"));
    }
}
