use super::*;

/// The spec's standard-layout table, pad note by pad note (GM drum map
/// shifted so C4 = pad note 0), plus the edges that have no role.
#[test]
fn standard_layout_inference_table() {
    use PadRole::*;
    let expected = [
        (0, Kick),
        (1, Rim),
        (2, Snare),
        (3, Clap),
        (4, Snare),
        (5, TomLow),
        (6, ClosedHat),
        (7, TomLow),
        (8, PedalHat),
        (9, TomMid),
        (10, OpenHat),
        (11, TomMid),
        (12, TomHigh),
        (13, Crash),
        (14, TomHigh),
        (15, Ride),
        (16, Crash),
        (17, Ride),
        (18, Shaker),
        (19, Crash),
        (20, Perc),
    ];
    for (pad_note, role) in expected {
        assert_eq!(
            PadRole::standard(pad_note),
            Some(role),
            "pad note {pad_note}"
        );
    }
    // Pad note 0 is GM 36 (bass drum 1): the table is the GM map shifted.
    assert_eq!(STANDARD_LAYOUT_GM_BASE, 36);
    for pad_note in [-36, -1, 21, 36, 51] {
        assert_eq!(PadRole::standard(pad_note), None, "pad note {pad_note}");
    }
}

#[test]
fn an_explicit_role_overrides_the_standard_layout() {
    assert_eq!(PadRole::effective(None, 2), Some(PadRole::Snare));
    assert_eq!(
        PadRole::effective(Some(PadRole::Clap), 2),
        Some(PadRole::Clap)
    );
    assert_eq!(
        PadRole::effective(Some(PadRole::Perc), 40),
        Some(PadRole::Perc)
    );
    assert_eq!(PadRole::effective(None, 40), None);
}

#[test]
fn keys_round_trip_and_match_serde() {
    for role in PadRole::ALL {
        assert_eq!(PadRole::from_key(role.key()), Some(role));
        assert_eq!(
            serde_json::to_string(&role).unwrap(),
            format!("\"{}\"", role.key())
        );
        assert_eq!(role.tag().len(), 2);
    }
    assert_eq!(PadRole::from_key("standard"), None);
}
