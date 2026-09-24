use super::*;
use crate::sequencer::{PatternSnapshot, SwingResolution, Timebase, TrackPatternData};

const SIXTEENTH: f64 = GROOVE_RESOLUTION_SIXTEENTH;

/// A 16-step, 16th-timebase, unswung pattern with no active steps.
fn pattern(num_steps: usize) -> TrackPatternData {
    let mut data = PatternSnapshot::new_default(1, &[])
        .track_pattern_data(0)
        .expect("default track data");
    data.clear_step_content();
    data.track_params.num_steps = num_steps;
    data.track_params.timebase = Timebase::Sixteenth;
    data.track_params.swing = 50.0;
    data.track_params.swing_resolution = SwingResolution::Sixteenth;
    data
}

fn activate(data: &mut TrackPatternData, step: usize, velocity: f32) {
    data.track_bits[step / 64] |= 1 << (step % 64);
    data.step_data[step][StepParam::Velocity.index()] = velocity;
}

/// A step Delay p-lock (the plain step-edit way to nudge a hit).
fn delayed_step(data: &mut TrackPatternData, step: usize, delay: f32, velocity: f32) {
    activate(data, step, velocity);
    data.step_data[step][StepParam::Delay.index()] = delay;
}

/// A captured hit, stored the way Capture MIDI stores it
/// (`app/retrospective.rs`): one chord note carrying its own delay.
fn chord_step(data: &mut TrackPatternData, step: usize, delays: &[f32], velocity: f32) {
    activate(data, step, velocity);
    for delay in delays {
        data.chord_snapshot.steps[step].push(0.0);
        data.chord_snapshot.durations[step].push(1.0);
        data.chord_snapshot.delays[step].push(*delay);
    }
}

fn hit(beat: f64, velocity: f32) -> HeardHit {
    HeardHit { beat, velocity }
}

fn options(period_beats: f64, resolution_beats: f64) -> GrooveExtractOptions {
    GrooveExtractOptions {
        name: "Pocket".to_string(),
        period_beats,
        resolution_beats,
    }
}

fn pad(pad_note: i32, hits: Vec<HeardHit>) -> GroovePadSource {
    GroovePadSource { pad_note, hits }
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "{what}: {actual} != {expected}"
    );
}

// --- snap -------------------------------------------------------------------

#[test]
fn snap_picks_the_nearest_slot_with_a_signed_offset() {
    // Slightly late on slot 1.
    let (slot, offset) = snap_to_slot(0.26, SIXTEENTH);
    assert_eq!(slot, 1);
    assert_close(offset as f64, 0.04, "late offset");
    // A kick pushed ahead of the beat: the recorder stores it as very late
    // on the previous step; nearest snap reads it as early on the right one.
    let (slot, offset) = snap_to_slot(0.24, SIXTEENTH);
    assert_eq!(slot, 1);
    assert_close(offset as f64, -0.04, "early offset");
    // Exactly halfway belongs to the NEXT slot, keeping offsets in [-0.5, 0.5).
    let (slot, offset) = snap_to_slot(0.125, SIXTEENTH);
    assert_eq!(slot, 1);
    assert_close(offset as f64, -0.5, "half-slot offset");
    // Before the pattern start (a pickup) stays signed and unfolded.
    let (slot, offset) = snap_to_slot(-0.01, SIXTEENTH);
    assert_eq!(slot, 0);
    assert_close(offset as f64, -0.04, "pickup offset");
    for beat in [0.0, 0.1, 0.37, 1.49, 3.99, 7.126] {
        let (_, offset) = snap_to_slot(beat, SIXTEENTH);
        assert!((-0.5..0.5).contains(&offset), "{beat} snapped to {offset}");
    }
}

#[test]
fn extraction_snaps_a_late_previous_step_hit_onto_the_next_slot_as_early() {
    // Kick on step 3 at delay 0.9 = heard 0.025 beats ahead of step 4.
    let mut kick = pattern(16);
    chord_step(&mut kick, 3, &[0.9], 1.0);
    let groove = extract_groove(
        1,
        &options(GROOVE_PERIOD_ONE_BAR, SIXTEENTH),
        &[pad(36, heard_hits(&kick))],
    )
    .expect("groove");
    let row = groove.pad_row(36).expect("kick row");
    assert_eq!(row.slots[4].source, GrooveSlotSource::Measured);
    assert_close(row.slots[4].offset as f64, -0.1, "early kick on slot 4");
    assert_ne!(
        row.slots[3].source,
        GrooveSlotSource::Measured,
        "not the floor slot"
    );
}

// --- heard positions --------------------------------------------------------

#[test]
fn heard_hits_read_step_delay_and_chord_delays() {
    let mut data = pattern(16);
    delayed_step(&mut data, 2, 0.2, 0.8);
    chord_step(&mut data, 5, &[0.1, 0.3], 0.6);
    // The scheduler ignores step Delay on chord steps; so does extraction.
    data.step_data[5][StepParam::Delay.index()] = 0.9;

    let hits = heard_hits(&data);
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert_close(hits[0].beat, 0.5 + 0.2 * 0.25, "Delay p-lock");
    assert_eq!(hits[0].velocity, 0.8);
    assert_close(hits[1].beat, 1.25 + 0.1 * 0.25, "first chord note");
    assert_close(hits[2].beat, 1.25 + 0.3 * 0.25, "second chord note");
    assert_eq!(hits[2].velocity, 0.6);
}

#[test]
fn heard_hits_follow_the_member_timebase() {
    // Eighth-note member: step 1 starts at 0.5 beats and a delay is a
    // fraction of an eighth, not of a 16th.
    let mut data = pattern(8);
    data.track_params.timebase = Timebase::Eighth;
    delayed_step(&mut data, 1, 0.5, 1.0);
    let hits = heard_hits(&data);
    assert_eq!(hits.len(), 1);
    assert_close(hits[0].beat, 0.5 + 0.5 * 0.5, "eighth-timebase delay");
}

#[test]
fn heard_hits_bake_in_track_swing_and_step_swing_overrides() {
    let mut data = pattern(16);
    data.track_params.swing = 66.0;
    for step in 0..4 {
        activate(&mut data, step, 1.0);
    }
    // Step 3 swings harder through a per-step swing p-lock.
    data.swing_plock_snapshot[3] = Some(75.0_f32.to_bits());

    let hits = heard_hits(&data);
    let swing_66 = (0.66 - 0.5) * 2.0 * 0.25;
    let swing_75 = (0.75 - 0.5) * 2.0 * 0.25;
    assert_close(hits[0].beat, 0.0, "even 16th is straight");
    assert_close(hits[1].beat, 0.25 + swing_66, "odd 16th swings");
    assert_close(hits[2].beat, 0.5, "even 16th is straight");
    assert_close(hits[3].beat, 0.75 + swing_75, "step swing p-lock overrides");

    let groove = extract_groove(
        1,
        &options(GROOVE_PERIOD_ONE_BAR, SIXTEENTH),
        &[pad(42, hits)],
    )
    .expect("groove");
    let row = groove.pad_row(42).expect("hat row");
    assert_close(
        row.slots[1].offset as f64,
        swing_66 / 0.25,
        "swing is part of the feel",
    );
    assert_close(row.slots[0].offset as f64, 0.0, "downbeat stays straight");
    // 75% swing lands exactly half a slot late: nearest snap reads it as the
    // next slot, half early.
    assert_eq!(row.slots[4].source, GrooveSlotSource::Measured);
    assert_close(row.slots[4].offset as f64, -0.5, "75% swing snaps forward");
}

#[test]
fn eighth_note_swing_only_delays_the_odd_eighth() {
    let mut data = pattern(16);
    data.track_params.swing = 60.0;
    data.track_params.swing_resolution = SwingResolution::Eighth;
    for step in [0, 1, 2, 3] {
        activate(&mut data, step, 1.0);
    }
    let beats = heard_hits(&data)
        .iter()
        .map(|hit| hit.beat)
        .collect::<Vec<_>>();
    let swing = (0.6 - 0.5) * 2.0 * 0.5;
    assert_close(beats[0], 0.0, "step 0");
    assert_close(beats[1], 0.25, "step 1 is in the first eighth");
    assert_close(beats[2], 0.5 + swing, "step 2 opens the odd eighth");
    assert_close(beats[3], 0.75 + swing, "step 3 is in the odd eighth");
}

// --- aggregation ------------------------------------------------------------

#[test]
fn slots_take_the_median_offset_mad_spread_and_accent_over_the_pad_median() {
    // One-bar period, four repeats of slot 4 (beat 1.0) at different offsets,
    // plus quieter hits elsewhere to pin the pad median.
    let offsets = [0.10, 0.20, 0.12, 0.30];
    let mut hits = offsets
        .iter()
        .enumerate()
        .map(|(repeat, offset)| hit(repeat as f64 * 4.0 + 1.0 + offset * 0.25, 1.0))
        .collect::<Vec<_>>();
    for repeat in 0..4 {
        hits.push(hit(repeat as f64 * 4.0 + 2.0, 0.5));
    }
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(38, hits)]).expect("groove");
    let row = groove.pad_row(38).expect("row");
    let slot = row.slots[4];
    // median(0.10, 0.12, 0.20, 0.30) = 0.16; |dev| = 0.06 0.04 0.04 0.14 -> 0.05.
    assert_close(slot.offset as f64, 0.16, "median offset");
    assert_close(slot.spread as f64, 0.05, "MAD spread");
    // Pad median velocity over (1,1,1,1,.5,.5,.5,.5) = 0.75.
    assert_close(slot.velocity_scale as f64, 1.0 / 0.75, "accent");
    assert_close(row.slots[8].velocity_scale as f64, 0.5 / 0.75, "unaccented");
    assert_close(row.slots[8].spread as f64, 0.0, "identical repeats");
}

#[test]
fn same_slot_collisions_keep_the_louder_hit_per_repeat() {
    // A flam on slot 4: a ghost well early, then the real, louder hit.
    let flam = vec![hit(1.0 - 0.2 * 0.25, 0.3), hit(1.0 + 0.1 * 0.25, 0.9)];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(38, flam)]).expect("groove");
    let slot = groove.pad_row(38).expect("row").slots[4];
    assert_close(slot.offset as f64, 0.1, "louder hit's timing");
    assert_close(slot.spread as f64, 0.0, "one sample, not two");
    assert_close(slot.velocity_scale as f64, 1.0, "louder hit's velocity");

    // Equal velocities: the hit nearer the grid wins, whatever the order.
    for hits in [
        vec![hit(1.0 + 0.3 * 0.25, 1.0), hit(1.0 - 0.05 * 0.25, 1.0)],
        vec![hit(1.0 - 0.05 * 0.25, 1.0), hit(1.0 + 0.3 * 0.25, 1.0)],
    ] {
        let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(38, hits)]).expect("groove");
        assert_close(
            groove.pad_row(38).unwrap().slots[4].offset as f64,
            -0.05,
            "nearer wins",
        );
    }

    // The same slot in DIFFERENT repeats is not a collision: both count.
    let repeats = vec![hit(1.0 + 0.1 * 0.25, 1.0), hit(5.0 + 0.3 * 0.25, 1.0)];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(38, repeats)]).expect("groove");
    let slot = groove.pad_row(38).unwrap().slots[4];
    assert_close(slot.offset as f64, 0.2, "median of both repeats");
    assert_close(slot.spread as f64, 0.1, "spread across repeats");
}

#[test]
fn two_bar_period_keeps_each_bar_and_one_bar_period_folds_them() {
    // Four bars of hats on beat 1 (slot 4 of each bar): bars 1 and 3 laid
    // back, bars 2 and 4 pushed.
    let hits = (0..4)
        .map(|bar| {
            let offset = if bar % 2 == 0 { 0.2 } else { -0.1 };
            hit(bar as f64 * 4.0 + 1.0 + offset * 0.25, 1.0)
        })
        .collect::<Vec<_>>();

    let two_bar = extract_groove(1, &options(8.0, SIXTEENTH), &[pad(42, hits.clone())])
        .expect("two-bar groove");
    assert_eq!(two_bar.slot_count(), 32);
    let row = two_bar.pad_row(42).expect("row");
    assert_close(row.slots[4].offset as f64, 0.2, "bar one");
    assert_close(row.slots[20].offset as f64, -0.1, "bar two");
    assert_eq!(row.slots[4].source, GrooveSlotSource::Measured);
    assert_eq!(row.slots[20].source, GrooveSlotSource::Measured);

    let one_bar =
        extract_groove(1, &options(4.0, SIXTEENTH), &[pad(42, hits)]).expect("one-bar groove");
    assert_eq!(one_bar.slot_count(), 16);
    // median(0.2, -0.1, 0.2, -0.1) = 0.05
    assert_close(
        one_bar.pad_row(42).unwrap().slots[4].offset as f64,
        0.05,
        "folded",
    );
}

#[test]
fn an_early_downbeat_at_the_pattern_end_folds_onto_slot_zero() {
    // A kick heard just before the loop point belongs to the NEXT downbeat.
    let hits = vec![hit(4.0 - 0.1 * 0.25, 1.0)];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(36, hits)]).expect("groove");
    let row = groove.pad_row(36).unwrap();
    assert_eq!(row.slots[0].source, GrooveSlotSource::Measured);
    assert_close(row.slots[0].offset as f64, -0.1, "early downbeat");
}

#[test]
fn shared_row_pools_every_pad_with_per_pad_velocity_normalization() {
    // Kick loud, hat quiet, both on slot 0 with different timing; hat also
    // on slot 2.
    let kick = vec![hit(0.0 + 0.1 * 0.25, 1.0)];
    let hat = vec![hit(0.0 + 0.3 * 0.25, 0.4), hit(0.5 + 0.2 * 0.25, 0.4)];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(36, kick), pad(42, hat)])
        .expect("groove");
    let shared = &groove.shared_row;
    assert_eq!(shared.slots[0].source, GrooveSlotSource::Measured);
    assert_close(shared.slots[0].offset as f64, 0.2, "median of kick and hat");
    assert_close(shared.slots[0].spread as f64, 0.1, "their spread");
    // Each hit is at its own pad's median, so nothing reads as an accent.
    assert_close(shared.slots[0].velocity_scale as f64, 1.0, "no fake accent");
    assert_close(shared.slots[2].offset as f64, 0.2, "hat only");
}

#[test]
fn pads_that_were_never_heard_get_no_row_and_fall_back_to_shared() {
    let groove = extract_groove(
        1,
        &options(4.0, SIXTEENTH),
        &[pad(36, vec![hit(0.0, 1.0)]), pad(38, Vec::new())],
    )
    .expect("groove");
    assert!(groove.pad_row(36).is_some());
    assert!(groove.pad_row(38).is_none());
    assert_eq!(groove.row_for_pad(38), &groove.shared_row);
    assert!(groove.is_well_formed());
}

#[test]
fn extraction_rejects_bad_grids_and_empty_sources() {
    assert_eq!(
        extract_groove(1, &options(4.0, 0.3), &[pad(36, vec![hit(0.0, 1.0)])]),
        Err(GrooveExtractError::InvalidGrid)
    );
    assert_eq!(
        extract_groove(1, &options(4.0, 0.0), &[pad(36, vec![hit(0.0, 1.0)])]),
        Err(GrooveExtractError::InvalidGrid)
    );
    assert_eq!(
        extract_groove(1, &options(4.0, SIXTEENTH), &[pad(36, Vec::new())]),
        Err(GrooveExtractError::NoHits)
    );
    let groove = extract_groove(
        7,
        &options(4.0, GROOVE_RESOLUTION_THIRTY_SECOND),
        &[pad(36, vec![hit(0.0, 1.0)])],
    )
    .expect("32nd grid");
    assert_eq!(groove.slot_count(), 32);
    assert_eq!(groove.id, 7);
    assert_eq!(groove.name, "Pocket");
}

// --- fill order -------------------------------------------------------------

#[test]
fn metric_classes_split_beats_ands_and_e_a() {
    assert_eq!(metric_class(0.0), 0);
    assert_eq!(metric_class(3.0), 0);
    assert_eq!(metric_class(1.5), 1);
    assert_eq!(metric_class(0.25), 2);
    assert_eq!(metric_class(2.75), 2);
    assert_eq!(metric_class(0.125), 3);
    assert_eq!(
        metric_class(1.0 / 3.0),
        metric_class(2.0 / 3.0),
        "triplets share a class"
    );
}

#[test]
fn fill_rule_one_copies_the_other_half_of_a_two_bar_period() {
    // Kick only in bar one on beat 1 (slot 4). Bar two's slot 20 is the same
    // position in the other half.
    let kick = vec![hit(1.0 + 0.2 * 0.25, 1.0)];
    let groove = extract_groove(1, &options(8.0, SIXTEENTH), &[pad(36, kick)]).expect("groove");
    let row = groove.pad_row(36).unwrap();
    assert_eq!(row.slots[20].source, GrooveSlotSource::FilledFromOtherHalf);
    assert_close(row.slots[20].offset as f64, 0.2, "copied from bar one");

    // A one-bar period has no other half: the same situation interpolates.
    let groove = extract_groove(
        1,
        &options(4.0, SIXTEENTH),
        &[pad(36, vec![hit(1.0 + 0.2 * 0.25, 1.0)])],
    )
    .expect("groove");
    assert_ne!(
        groove.pad_row(36).unwrap().slots[12].source,
        GrooveSlotSource::FilledFromOtherHalf
    );
}

#[test]
fn fill_rule_two_interpolates_between_same_class_neighbours() {
    // Hats measured on the "e" of beats 1 and 3 (slots 1 and 9, class e/a),
    // and on-beat hits that must NOT be interpolated from.
    let hat = vec![
        hit(0.25 + 0.1 * 0.25, 1.0),
        hit(2.25 + 0.3 * 0.25, 0.5),
        hit(1.0 - 0.4 * 0.25, 1.0),
    ];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(42, hat)]).expect("groove");
    let row = groove.pad_row(42).unwrap();
    // Slot 3 ("a" of beat 1) is class e/a: 2 slots after slot 1, 6 before
    // slot 9 -> t = 2/8.
    assert_eq!(row.slots[3].source, GrooveSlotSource::FilledFromNeighbors);
    assert_close(
        row.slots[3].offset as f64,
        0.1 + (0.3 - 0.1) * 0.25,
        "interpolated offset",
    );
    let pad_median = 1.0; // median(1.0, 0.5, 1.0)
    let expected_vel = (1.0 + (0.5 - 1.0) * 0.25) / pad_median;
    assert_close(
        row.slots[3].velocity_scale as f64,
        expected_vel,
        "interpolated accent",
    );
    // Wraps around the loop: slot 13 lies between 9 and 1 (+16).
    assert_eq!(row.slots[13].source, GrooveSlotSource::FilledFromNeighbors);
    assert_close(
        row.slots[13].offset as f64,
        0.3 + (0.1 - 0.3) * 0.5,
        "wrapped",
    );
    // On-beat class has only slot 4 measured: it fills every other beat.
    assert_eq!(row.slots[8].source, GrooveSlotSource::FilledFromNeighbors);
    assert_close(row.slots[8].offset as f64, -0.4, "same-class only");
}

#[test]
fn fill_rule_three_uses_the_shared_row_then_rule_four_zeroes() {
    // The kick is only ever heard on-beat, so its "&" slots have no
    // same-class neighbour; the hat plays the "&" of beat 1.
    let kick = vec![hit(0.0 + 0.1 * 0.25, 1.0)];
    let hat = vec![hit(0.5 + 0.25 * 0.25, 1.0)];
    let groove = extract_groove(1, &options(4.0, SIXTEENTH), &[pad(36, kick), pad(42, hat)])
        .expect("groove");
    let kick_row = groove.pad_row(36).unwrap();
    // Slot 2 is the "&": shared row measured it from the hat.
    assert_eq!(kick_row.slots[2].source, GrooveSlotSource::FilledFromShared);
    assert_close(kick_row.slots[2].offset as f64, 0.25, "from the shared row");
    // The shared row itself fills slot 6 (another "&") by interpolation, so
    // the kick takes that too.
    assert_eq!(
        groove.shared_row.slots[6].source,
        GrooveSlotSource::FilledFromNeighbors
    );
    assert_eq!(kick_row.slots[6].source, GrooveSlotSource::FilledFromShared);
    // Nobody played an e/a position: straight and neutral.
    assert_eq!(kick_row.slots[1].source, GrooveSlotSource::Zero);
    assert_eq!(kick_row.slots[1], GrooveSlot::default());
    assert_eq!(groove.shared_row.slots[1].source, GrooveSlotSource::Zero);
}

#[test]
fn fill_order_prefers_other_half_over_neighbours_over_shared() {
    // Two-bar period. Kick: slot 4 (bar 1) and slot 8 (bar 1) measured.
    // Slot 20 has an other-half value (slot 4) AND on-beat neighbours; the
    // other half must win.
    let kick = vec![hit(1.0 + 0.2 * 0.25, 1.0), hit(2.0 - 0.2 * 0.25, 1.0)];
    let hat = vec![hit(5.0 + 0.4 * 0.25, 1.0)];
    let groove = extract_groove(1, &options(8.0, SIXTEENTH), &[pad(36, kick), pad(42, hat)])
        .expect("groove");
    let kick_row = groove.pad_row(36).unwrap();
    assert_eq!(
        kick_row.slots[20].source,
        GrooveSlotSource::FilledFromOtherHalf
    );
    assert_close(kick_row.slots[20].offset as f64, 0.2, "other half wins");
    // Slot 28 (beat 7): other half (slot 12) unmeasured -> same-class
    // neighbours (slot 8 before, slot 4 after the wrap), not the shared row.
    assert_eq!(
        kick_row.slots[28].source,
        GrooveSlotSource::FilledFromNeighbors
    );
    // Measured always wins.
    assert_eq!(kick_row.slots[4].source, GrooveSlotSource::Measured);
}

// --- quantize source ---------------------------------------------------------

/// Where a straight hit at `beat` lands through a groove row.
fn through_groove(groove: &ProjectGroove, pad_note: i32, beat: f64) -> f64 {
    let row = groove.row_for_pad(pad_note);
    let slots = row.slots.len() as f64;
    let position = (beat.rem_euclid(groove.period_beats)) / groove.resolution_beats;
    let slot = (position.round() as usize) % slots as usize;
    beat + row.slots[slot].offset as f64 * groove.resolution_beats
}

#[test]
fn quantize_zeroes_delays_and_swing_and_moves_late_hits_to_their_nearest_step() {
    let mut data = pattern(16);
    data.track_params.swing = 62.0;
    data.swing_plock_snapshot[6] = Some(70.0_f32.to_bits());
    delayed_step(&mut data, 0, 0.1, 1.0); // late on step 0
    chord_step(&mut data, 3, &[0.9], 0.7); // early kick for step 4
    chord_step(&mut data, 8, &[0.2, 0.3], 0.9); // captured double
    data.step_data[8][StepParam::Transpose.index()] = 5.0;

    assert!(quantize_groove_source(&mut data));
    assert_eq!(data.track_params.swing, 50.0);
    assert!(data.swing_plock_snapshot.iter().all(Option::is_none));
    assert!(data
        .step_data
        .iter()
        .all(|params| params[StepParam::Delay.index()] == 0.0));
    assert!(data
        .chord_snapshot
        .delays
        .iter()
        .flatten()
        .all(|delay| *delay == 0.0));
    let active = (0..16)
        .filter(|step| step_active(&data, *step))
        .collect::<Vec<_>>();
    assert_eq!(active, vec![0, 4, 8], "the early kick moved onto step 4");
    assert_eq!(data.chord_snapshot.steps[4].len(), 1);
    assert_eq!(
        data.step_data[4][StepParam::Velocity.index()],
        0.7,
        "moved with its params"
    );
    assert!(data.chord_snapshot.steps[3].is_empty());
    assert_eq!(
        data.chord_snapshot.steps[8].len(),
        2,
        "chord notes stay together"
    );
    assert_eq!(
        data.step_data[8][StepParam::Transpose.index()],
        5.0,
        "params untouched"
    );

    assert!(
        !quantize_groove_source(&mut data),
        "quantizing twice is a no-op"
    );
}

#[test]
fn quantize_does_not_move_onto_an_occupied_step_or_with_device_locks() {
    let mut data = pattern(16);
    chord_step(&mut data, 3, &[0.8], 1.0);
    activate(&mut data, 4, 1.0); // already occupied
    chord_step(&mut data, 10, &[0.2, 0.9], 1.0); // not ALL notes late
    chord_step(&mut data, 12, &[0.7], 1.0);
    data.timebase_plock_snapshot[12] = Some(Timebase::Eighth as u32);
    assert!(quantize_groove_source(&mut data));
    let active = (0..16)
        .filter(|step| step_active(&data, *step))
        .collect::<Vec<_>>();
    assert_eq!(active, vec![3, 4, 10, 12], "nothing could move");
    assert!(data
        .chord_snapshot
        .delays
        .iter()
        .flatten()
        .all(|delay| *delay == 0.0));
}

#[test]
fn quantize_moves_a_last_step_pickup_onto_the_downbeat() {
    let mut data = pattern(16);
    chord_step(&mut data, 15, &[0.95], 1.0);
    assert!(quantize_groove_source(&mut data));
    assert!(step_active(&data, 0));
    assert!(!step_active(&data, 15));
}

#[test]
fn quantized_source_through_its_groove_sounds_as_before() {
    // A two-bar take repeated twice with a consistent pocket per pad: kick
    // pushed on the downbeats, snare dragging, swung hats. Consistent repeats
    // make the slot medians exact, so the equivalence is exact.
    let mut kick = pattern(32);
    let mut snare = pattern(32);
    let mut hat = pattern(32);
    hat.track_params.swing = 58.0;
    for bar in 0..2 {
        let base = bar * 16;
        // Kick early on beat 1 (stored late on the previous step), plus a
        // late kick on the "&" of 3.
        chord_step(&mut kick, (base + 15) % 32, &[0.88], 1.0);
        chord_step(&mut kick, base + 10, &[0.12], 0.8);
        // Snare drags on 2 and 4 via step Delay.
        delayed_step(&mut snare, base + 4, 0.18, 0.9);
        delayed_step(&mut snare, base + 12, 0.22, 1.0);
        // Hats on every 8th plus a ghost 16th, swung by the track.
        for step in (0..16).step_by(2) {
            delayed_step(&mut hat, base + step, 0.06, 0.6);
        }
        chord_step(&mut hat, base + 7, &[0.1], 0.3);
    }
    let sources = [(36, &kick), (38, &snare), (42, &hat)];
    let before = sources
        .iter()
        .map(|(note, data)| (*note, heard_hits(data)))
        .collect::<Vec<_>>();
    let groove = extract_groove(
        1,
        &options(8.0, SIXTEENTH),
        &before
            .iter()
            .map(|(note, hits)| pad(*note, hits.clone()))
            .collect::<Vec<_>>(),
    )
    .expect("groove");

    for ((note, data), (_, heard_before)) in sources.iter().zip(&before) {
        let mut quantized = (*data).clone();
        quantize_groove_source(&mut quantized);
        let straight = heard_hits(&quantized);
        assert_eq!(
            straight.len(),
            heard_before.len(),
            "pad {note} keeps every hit"
        );
        let mut replayed = straight
            .iter()
            .map(|hit| through_groove(&groove, *note, hit.beat).rem_euclid(8.0))
            .collect::<Vec<_>>();
        let mut original = heard_before
            .iter()
            .map(|hit| hit.beat.rem_euclid(8.0))
            .collect::<Vec<_>>();
        replayed.sort_by(f64::total_cmp);
        original.sort_by(f64::total_cmp);
        for (replayed, original) in replayed.iter().zip(&original) {
            let error = (replayed - original)
                .abs()
                .min(8.0 - (replayed - original).abs());
            assert!(error < 1e-5, "pad {note}: {replayed} vs {original}");
        }
    }
}

// --- settings ---------------------------------------------------------------

#[test]
fn settings_default_to_timing_only_and_sanitize_into_range() {
    let settings = RackGrooveSettings::default();
    assert_eq!(settings.active, None);
    assert_eq!(settings.timing_amount, 1.0);
    assert_eq!(settings.velocity_amount, 0.0);
    assert_eq!(settings.random_amount, 0.0);
    assert!(settings.is_default());

    let mut wild = RackGrooveSettings {
        active: Some(GrooveRef::Builtin("mpc-16-62".to_string())),
        timing_amount: 9.0,
        velocity_amount: -1.0,
        random_amount: f32::NAN,
    };
    wild.sanitize();
    assert_eq!(wild.timing_amount, GROOVE_TIMING_AMOUNT_MAX);
    assert_eq!(wild.velocity_amount, 0.0);
    assert_eq!(wild.random_amount, 0.0);

    let json = serde_json::to_string(&wild).expect("serialize");
    let restored: RackGrooveSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored, wild);
    let partial: RackGrooveSettings = serde_json::from_str("{}").expect("empty object");
    assert_eq!(partial, RackGrooveSettings::default());
}
