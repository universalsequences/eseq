pub const FADER_MIN_DB: f32 = -60.0;
pub const FADER_MAX_DB: f32 = 6.0;
pub const FADER_UNITY_DB: f32 = 0.0;

pub fn db_to_fader(db: f32) -> f32 {
    ((db - FADER_MIN_DB) / (FADER_MAX_DB - FADER_MIN_DB)).clamp(0.0, 1.0)
}

pub fn default_fader() -> f32 {
    db_to_fader(FADER_UNITY_DB)
}

pub fn fader_to_db(fader: f32) -> f32 {
    FADER_MIN_DB + fader.clamp(0.0, 1.0) * (FADER_MAX_DB - FADER_MIN_DB)
}

pub fn fader_to_gain(fader: f32) -> f32 {
    let fader = fader.clamp(0.0, 1.0);
    if fader <= 0.0 {
        0.0
    } else {
        10.0_f32.powf(fader_to_db(fader) / 20.0)
    }
}

pub fn fader_db_label(fader: f32) -> String {
    let fader = fader.clamp(0.0, 1.0);
    if fader <= 0.0 {
        "-inf".to_string()
    } else {
        format!("{:+}dB", fader_to_db(fader).round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_defaults_below_max_fader() {
        let fader = default_fader();
        assert!((fader - (60.0 / 66.0)).abs() < 0.0001);
        assert!((fader_to_gain(fader) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn max_fader_maps_to_plus_six_db() {
        assert!((fader_to_db(1.0) - 6.0).abs() < 0.0001);
        assert!((fader_to_gain(1.0) - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.0001);
    }

    #[test]
    fn zero_fader_is_silent() {
        assert_eq!(fader_to_gain(0.0), 0.0);
        assert_eq!(fader_db_label(0.0), "-inf");
    }
}
