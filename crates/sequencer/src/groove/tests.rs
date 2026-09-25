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
    GroovePadSource {
        pad_note,
        role: None,
        hits,
    }
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
    assert_eq!(groove.row_for_pad(38, None), &groove.shared_row);
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
    let row = groove.row_for_pad(pad_note, None);
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
        active: Some(7),
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

// --- application (eseq-groove.2) --------------------------------------------

fn row_of(offsets: &[f32]) -> GrooveRow {
    GrooveRow {
        slots: offsets
            .iter()
            .map(|&offset| GrooveSlot {
                offset,
                ..GrooveSlot::default()
            })
            .collect(),
    }
}

fn track_groove(period: f64, resolution: f64, offsets: &[f32]) -> TrackGrooveSnapshot {
    TrackGrooveSnapshot {
        period_beats: period,
        resolution_beats: resolution,
        row: std::sync::Arc::new(row_of(offsets)),
        timing_amount: 1.0,
        velocity_amount: 0.0,
        random_amount: 0.0,
        pad_note: 0,
    }
}

/// `offset_beats` reads the slot at the transport beat, interpolates between
/// slots (a 16th groove shapes 32nds), wraps at the period and scales by the
/// timing amount.
#[test]
fn applied_offset_interpolates_wraps_and_scales_by_timing_amount() {
    let groove = track_groove(1.0, 0.25, &[0.0, 0.4, 0.2, 0.0]);
    let at = |beats: f64| groove.offset_beats(beats);
    assert_eq!(at(0.0), 0.0);
    assert!((at(0.25) - 0.1).abs() < 1e-6, "slot 1: 0.4 of a 16th");
    assert!((at(0.5) - 0.05).abs() < 1e-6, "slot 2");
    // A 32nd between slots 1 and 2: halfway between 0.4 and 0.2 slots.
    assert!((at(0.375) - 0.3 * 0.25).abs() < 1e-6);
    // Between slot 3 and the wrap back to slot 0.
    assert!((at(0.875) - 0.0).abs() < 1e-6);
    // Period wrap, bar 7: same pocket.
    assert!((at(7.25) - at(0.25)).abs() < 1e-12);
    // A boundary a hair either side of a slot reads as that slot.
    assert!((at(0.25 - 1e-9) - at(0.25)).abs() < 1e-12);
    assert!((at(0.25 + 1e-9) - at(0.25)).abs() < 1e-12);
    assert!(
        (at(4.0 - 1e-10) - at(0.0)).abs() < 1e-12,
        "wrap edge snaps to slot 0"
    );

    let mut half = groove.clone();
    half.timing_amount = 0.5;
    assert!((half.offset_beats(0.25) - 0.05).abs() < 1e-6);
    let mut off = groove.clone();
    off.timing_amount = 0.0;
    assert_eq!(off.offset_beats(0.25), 0.0);
}

/// Early offsets (eseq-groove.3): negative slots move a trig BEFORE its
/// straight boundary, interpolation passes through zero between an early and
/// a late slot, and degenerate inputs never move a trig.
#[test]
fn applied_offset_is_signed_and_early_slots_move_trigs_early() {
    let groove = track_groove(1.0, 0.25, &[-0.3, 0.2, -0.1, 0.0]);
    assert!(
        (groove.offset_beats(0.0) + 0.3 * 0.25).abs() < 1e-7,
        "early kick"
    );
    assert!((groove.offset_beats(0.5) + 0.1 * 0.25).abs() < 1e-7);
    assert!(groove.offset_beats(0.25) > 0.0);
    // Halfway between -0.3 and 0.2 slots: -0.05 of a 16th.
    assert!((groove.offset_beats(0.125) + 0.05 * 0.25).abs() < 1e-7);
    assert_eq!(groove_offset_samples(&groove, 0.0, 24_000.0), -1_800);
    assert_eq!(groove_offset_samples(&groove, 0.25, 24_000.0), 1_200);
    assert_eq!(groove_offset_samples(&groove, 0.5, 24_000.0), -600);
    assert_eq!(
        grooved_sample_time(&groove, 0.25, 6_000, 24_000.0, GrooveFloor::at(0)),
        Some(7_200)
    );
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, GrooveFloor::at(0)),
        Some(22_200)
    );
    assert_eq!(
        grooved_sample_time(&groove, 1.5, 36_000, 24_000.0, GrooveFloor::at(0)),
        Some(35_400)
    );
    // Degenerate inputs never move a trig.
    assert_eq!(groove_offset_samples(&groove, f64::NAN, 24_000.0), 0);
    assert_eq!(groove_offset_samples(&groove, 0.25, 0.0), 0);
    assert_eq!(track_groove(1.0, 0.25, &[]).offset_beats(0.25), 0.0);
    let mut nan = groove.clone();
    nan.timing_amount = f32::NAN;
    assert_eq!(
        nan.offset_beats(0.0),
        0.0,
        "a NaN amount is no move, not the cap"
    );
}

/// The audio-frontier floor: an early offset never lands before
/// `not_before`, never delays a trig that was already before it, and never
/// touches a late offset.
#[test]
fn grooved_sample_time_floors_early_offsets_at_the_frontier() {
    let groove = track_groove(1.0, 0.25, &[-0.3, 0.2, -0.1, 0.0]);
    // Straight 24_000, groove wants 22_200.
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, GrooveFloor::at(22_200)),
        Some(22_200)
    );
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, GrooveFloor::at(23_000)),
        Some(23_000)
    );
    // A frontier past the straight sample: the trig was already late, the
    // floor never makes it later.
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, GrooveFloor::at(30_000)),
        Some(24_000)
    );
    // Late offsets ignore the floor entirely.
    assert_eq!(
        grooved_sample_time(&groove, 0.25, 6_000, 24_000.0, GrooveFloor::at(9_000)),
        Some(7_200)
    );
    // Transport start: an early downbeat cannot go below sample 0.
    assert_eq!(
        grooved_sample_time(&groove, 0.0, 0, 24_000.0, GrooveFloor::at(0)),
        Some(0)
    );
}

/// The resync dedupe: after a mid-play resync rewinds the clock to the audio
/// frontier, a trig discovered before the resync whose early move lands
/// before the frontier has already sounded, so it is dropped instead of
/// clamped (which would play it twice). Outside the replayed window, or when
/// the move still lands at or after the frontier, it plays as usual.
#[test]
fn grooved_sample_time_drops_early_hits_already_played_before_a_resync() {
    let groove = track_groove(1.0, 0.25, &[-0.3, 0.2, -0.1, 0.0]);
    let resynced = |not_before, replayed_until| GrooveFloor {
        not_before,
        replayed_until,
    };
    // Straight 24_000 wants 22_200; the resync landed at 23_000 with the
    // old frontier at 30_000: the hit played at 22_200 already.
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, resynced(23_000, 30_000)),
        None
    );
    // Not yet played (lands at or after the frontier): re-enqueued as usual.
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, resynced(22_000, 30_000)),
        Some(22_200)
    );
    // Never discovered before the resync (straight past the old frontier):
    // it cannot have sounded, so it clamps like a transport start.
    assert_eq!(
        grooved_sample_time(&groove, 1.0, 24_000, 24_000.0, resynced(23_000, 24_000)),
        Some(23_000)
    );
    // Late offsets are never dropped.
    assert_eq!(
        grooved_sample_time(&groove, 0.25, 6_000, 24_000.0, resynced(9_000, 30_000)),
        Some(7_200)
    );
}

/// `max_early_beats` is the most negative applied offset as a lead: zero for
/// a late-only groove, scaled by timing, widened by Random spread, and never
/// past the 0.75-slot cap (which also bounds the applied offset itself).
#[test]
fn max_early_beats_bounds_every_applied_offset() {
    assert_eq!(
        track_groove(1.0, 0.25, &[0.0, 0.3, 0.1, 0.2]).max_early_beats(),
        0.0
    );
    assert_eq!(track_groove(1.0, 0.25, &[]).max_early_beats(), 0.0);
    let groove = track_groove(1.0, 0.25, &[-0.3, 0.2, -0.1, 0.0]);
    assert!((groove.max_early_beats() - 0.3 * 0.25).abs() < 1e-7);
    let mut heavy = groove.clone();
    heavy.timing_amount = 1.5;
    assert!((heavy.max_early_beats() - 0.45 * 0.25).abs() < 1e-7);
    // A negative amount flips late slots early.
    let mut flipped = groove.clone();
    flipped.timing_amount = -1.0;
    assert!((flipped.max_early_beats() - 0.2 * 0.25).abs() < 1e-7);
    // Random widens the lead by `random * spread` slots.
    let mut jittery = accented_groove(&[-0.3, 0.2, -0.1, 0.0], &[1.0; 4], &[0.2; 4]);
    jittery.random_amount = 1.0;
    jittery.pad_note = 36;
    assert!((jittery.max_early_beats() - 0.5 * 0.25).abs() < 1e-7);
    // The bound holds for every position and bar.
    for groove in [&groove, &heavy, &flipped, &jittery] {
        let lead = groove.max_early_beats();
        for index in 0..(64 * 8) {
            let beats = index as f64 / 32.0;
            assert!(
                groove.offset_beats(beats) >= -lead - 1e-12,
                "{beats}: {} vs lead {lead}",
                groove.offset_beats(beats)
            );
        }
    }
    // The cap: E <= 0.75 * resolution, and no applied offset goes earlier.
    let mut capped = accented_groove(&[-0.49, -0.49, -0.49, -0.49], &[1.0; 4], &[0.5; 4]);
    capped.timing_amount = 1.5;
    capped.random_amount = 1.0;
    assert!((capped.max_early_beats() - MAX_EARLY_SLOTS * 0.25).abs() < 1e-12);
    for index in 0..64 {
        assert!(capped.offset_beats(index as f64 * 0.25) >= -MAX_EARLY_SLOTS * 0.25 - 1e-12);
    }
    // The table lead is the largest track lead; `None` tracks add nothing.
    assert_eq!(max_early_lead_beats(&[]), 0.0);
    assert_eq!(
        max_early_lead_beats(&[None, Some(groove.clone()), Some(heavy.clone()), None]),
        heavy.max_early_beats()
    );
}

/// The MPC swings (factory library content) are two-slot grooves that delay
/// exactly what track swing delays at the same percentage and resolution.
#[test]
fn mpc_swings_match_track_swing_delays() {
    let samples_per_quarter = 24_000.0;
    for resolution in MPC_SWING_RESOLUTIONS {
        for percent in MPC_SWING_PERCENTS {
            let groove = mpc_swing_groove(percent, resolution);
            let stem = mpc_swing_file_stem(percent, resolution);
            assert!(groove.is_well_formed(), "{stem}");
            assert!(groove.pad_rows.is_empty());
            assert_eq!(groove.slot_count(), 2);
            let snapshot = TrackGrooveSnapshot {
                period_beats: groove.period_beats,
                resolution_beats: groove.resolution_beats,
                row: std::sync::Arc::new(groove.shared_row.clone()),
                timing_amount: 1.0,
                velocity_amount: 0.0,
                random_amount: 0.0,
                pad_note: 0,
            };
            let res = groove.resolution_beats;
            let swing =
                (((percent as f64 / 100.0) - 0.5) * 2.0 * res * samples_per_quarter).round();
            for bucket in 0..8u64 {
                let beats = bucket as f64 * res;
                let expected = if bucket % 2 == 1 { swing as u64 } else { 0 };
                assert_eq!(
                    groove_offset_samples(&snapshot, beats, samples_per_quarter),
                    expected as i64,
                    "{stem} bucket {bucket}"
                );
            }
        }
    }
    assert_eq!(mpc_swing_file_stem(58, 0.25), "mpc-swing-58-16th");
    assert_eq!(mpc_swing_file_stem(66, 0.5), "mpc-swing-66-8th");
}

/// The rev-1 built-in MPC swings are factory library files now: every file
/// in `content/grooves/mpc-swing-*` is exactly the generator's output, and
/// every generated swing ships.
#[test]
fn factory_mpc_swing_files_match_the_generator() {
    let factory = crate::app_paths::app_paths().grooves_dir();
    let listed = library::list_groove_library_in(&factory, &factory.join("no-user-tier"));
    for resolution in MPC_SWING_RESOLUTIONS {
        for percent in MPC_SWING_PERCENTS {
            let stem = mpc_swing_file_stem(percent, resolution);
            let entry = listed
                .iter()
                .find(|entry| entry.stem == stem)
                .unwrap_or_else(|| panic!("factory groove {stem} is missing"));
            assert_eq!(entry.tier, GrooveLibraryTier::Factory);
            let groove = library::read_groove_file(&entry.path).expect("factory file reads");
            assert_eq!(groove, mpc_swing_groove(percent, resolution), "{stem}");
            assert_eq!(entry.name, groove.name);
        }
    }
}

/// Writes the factory MPC swing files from the generator. Run once after
/// changing `mpc_swing_groove` (`--run-ignored only`), then commit them.
#[test]
#[ignore = "regenerates content/grooves; run explicitly"]
fn regenerate_factory_mpc_swing_files() {
    let factory = crate::app_paths::app_paths().grooves_dir();
    std::fs::create_dir_all(&factory).expect("factory grooves dir");
    for resolution in MPC_SWING_RESOLUTIONS {
        for percent in MPC_SWING_PERCENTS {
            let path = factory.join(format!(
                "{}.{GROOVE_FILE_EXTENSION}",
                mpc_swing_file_stem(percent, resolution)
            ));
            library::write_groove_file(&path, &mpc_swing_groove(percent, resolution))
                .expect("write factory groove");
        }
    }
}

/// The scheduler table: each member of a rack with an active pool groove
/// gets its pad's row by pad note, else the shared row (also for a member
/// with no pad); tracks outside racks, and racks with no (or a dangling)
/// active groove, get `None`.
#[test]
fn track_groove_snapshots_resolve_pad_rows_by_note_else_shared() {
    use crate::project::{ProjectRackConfig, ProjectRackPad};
    let mut groove = mpc_swing_groove(50, 0.25);
    groove.id = 7;
    groove.period_beats = 0.5;
    groove.pad_rows = vec![GroovePadRow {
        pad_note: 42,
        role: None,
        row: row_of(&[0.1, 0.3]),
    }];
    let rack = ProjectRackConfig {
        pads: vec![ProjectRackPad::new(36, 0), ProjectRackPad::new(42, 1)],
        groove: RackGrooveSettings {
            active: Some(7),
            timing_amount: 0.75,
            velocity_amount: 0.5,
            random_amount: 0.25,
        },
        ..Default::default()
    };
    let mut swing = mpc_swing_groove(66, 0.25);
    swing.id = 3;
    let pool = vec![swing, groove.clone()];
    // Members: track 3 = kick (36), track 1 = hat (42), track 4 = no pad.
    let members = vec![3usize, 1, 4];
    let table = track_groove_snapshots([(members.as_slice(), &rack)], &pool, 6);
    assert_eq!(table.len(), 6);
    for track in [0usize, 2, 5] {
        assert!(table[track].is_none(), "track {track} is outside the rack");
    }
    let kick = table[3].as_ref().expect("kick groove");
    let hat = table[1].as_ref().expect("hat groove");
    let loose = table[4].as_ref().expect("member without a pad");
    assert_eq!(*kick.row, groove.shared_row, "no kick row: shared");
    assert_eq!(*hat.row, row_of(&[0.1, 0.3]), "hat plays its own row");
    assert_eq!(*loose.row, groove.shared_row);
    assert_eq!(kick.period_beats, 0.5);
    assert_eq!(kick.resolution_beats, 0.25);
    assert_eq!(kick.timing_amount, 0.75);
    assert_eq!(kick.velocity_amount, 0.5);
    assert_eq!(kick.random_amount, 0.25);
    // Random is seeded by pad note; a padless member gets a key outside the
    // pad-note domain, distinct per member.
    assert_eq!(kick.pad_note, 36);
    assert_eq!(hat.pad_note, 42);
    assert_eq!(loose.pad_note, padless_seed_key(2));
    assert!(loose.pad_note < -1000);
    assert_ne!(padless_seed_key(2), padless_seed_key(3));

    // Pad rows follow the NOTE: swap the notes and the hat row moves.
    let mut swapped = rack.clone();
    swapped.pads[0].pad_note = 42;
    swapped.pads[1].pad_note = 36;
    let table = track_groove_snapshots([(members.as_slice(), &swapped)], &pool, 6);
    assert_eq!(*table[3].as_ref().unwrap().row, row_of(&[0.1, 0.3]));
    assert_eq!(*table[1].as_ref().unwrap().row, groove.shared_row);

    // A shared-row-only pool groove (an applied MPC swing) plays its shared
    // row on every member.
    let mut swing = rack.clone();
    swing.groove.active = Some(3);
    let table = track_groove_snapshots([(members.as_slice(), &swing)], &pool, 6);
    for track in [3usize, 1, 4] {
        assert_eq!(*table[track].as_ref().unwrap().row, pool[0].shared_row);
    }

    // Off, or an id not in the pool: nothing.
    for active in [None, Some(99)] {
        let mut off = rack.clone();
        off.groove.active = active;
        let table = track_groove_snapshots([(members.as_slice(), &off)], &pool, 6);
        assert!(table.iter().all(Option::is_none));
    }
    // The same rack against an empty pool plays straight.
    let table = track_groove_snapshots([(members.as_slice(), &rack)], &[], 6);
    assert!(table.iter().all(Option::is_none));
    // A member index past the table is ignored rather than panicking.
    let table = track_groove_snapshots([(&[9usize][..], &rack)], &pool, 6);
    assert!(table.iter().all(Option::is_none));
}

// --- moving grooves between racks (eseq-groove.7) ----------------------------

/// A two-slot 16th groove with rows for `pad_notes` (each row distinct) and
/// a shared row of `[0.0, shared]`.
fn two_slot_groove(id: GrooveId, name: &str, shared: f32, pad_notes: &[i32]) -> ProjectGroove {
    let mut groove = mpc_swing_groove(50, 0.25);
    groove.id = id;
    groove.name = name.to_string();
    groove.shared_row = row_of(&[0.0, shared]);
    groove.pad_rows = pad_notes
        .iter()
        .map(|&pad_note| GroovePadRow {
            pad_note,
            role: None,
            row: row_of(&[0.01 * pad_note as f32, 0.3]),
        })
        .collect();
    groove
}

fn rack_with_pads(notes: &[i32]) -> crate::project::ProjectRackConfig {
    crate::project::ProjectRackConfig {
        pads: notes
            .iter()
            .enumerate()
            .map(|(member, &pad_note)| crate::project::ProjectRackPad::new(pad_note, member))
            .collect(),
        choke_groups: vec![None; notes.len()],
        ..Default::default()
    }
}

/// Copy-on-apply into the pool: a new groove takes the next pool id (the
/// incoming id means nothing here), a groove with the same feel as a pool
/// groove reuses it instead of duplicating, and a malformed one is refused.
#[test]
fn import_into_pool_takes_fresh_ids_reuses_same_feel_and_skips_malformed() {
    let mut pool = vec![two_slot_groove(1, "Own", 0.2, &[36])];
    let mut broken = two_slot_groove(5, "Broken", 0.1, &[]);
    broken.shared_row.slots.pop();
    let incoming = vec![
        two_slot_groove(1, "Dilla", 0.4, &[42]),
        broken.clone(),
        // Same feel as the pool's own groove, different id: reused.
        two_slot_groove(9, "Own", 0.2, &[36]),
    ];
    let map = import_grooves(&mut pool, &incoming);
    assert_eq!(map, vec![(1, 2), (9, 1)], "fresh id for Dilla, Own reused");
    assert_eq!(pool.len(), 2);
    let dilla = pool_groove(&pool, 2).expect("Dilla imported");
    assert_eq!(dilla.name, "Dilla");
    assert!(dilla.same_feel(&incoming[0]));
    assert_eq!(
        pool_groove(&pool, 1).unwrap().name,
        "Own",
        "the pool's own groove is untouched"
    );
    assert_eq!(import_groove(&mut pool, &broken), None);

    // Importing the same set again changes nothing.
    let again = import_grooves(&mut pool, &incoming);
    assert_eq!(again, map);
    assert_eq!(pool.len(), 2, "re-importing does not pile up copies");
    // A renamed copy is a different groove (the name is part of the feel a
    // user picks by), so it lands beside the original.
    let mut renamed = two_slot_groove(9, "Own", 0.2, &[36]);
    renamed.name = "Own (edit)".to_string();
    assert_eq!(import_groove(&mut pool, &renamed), Some(3));
    assert_eq!(next_pool_groove_id(&pool), 4);
    assert_eq!(next_pool_groove_id(&[]), 1);
}

/// A kit's groove copy installs its amounts (sanitized) under the pool id
/// its copy landed at; a rack's settings export as a copy of its active
/// pool groove, or nothing when it plays none.
#[test]
fn kit_groove_copies_the_active_pool_groove_and_its_amounts() {
    let pool = vec![
        two_slot_groove(1, "Dilla", 0.2, &[36]),
        two_slot_groove(4, "Madlib", 0.4, &[]),
    ];
    let settings = RackGrooveSettings {
        active: Some(4),
        timing_amount: 1.25,
        velocity_amount: 0.5,
        random_amount: 0.1,
    };
    let kit = KitGroove::from_rack(&settings, &pool).expect("an active groove travels");
    assert_eq!(kit.groove, pool[1]);
    assert_eq!(kit.timing_amount, 1.25);
    assert_eq!(
        kit.settings(Some(7)),
        RackGrooveSettings {
            active: Some(7),
            ..settings.clone()
        }
    );
    let wild = KitGroove {
        velocity_amount: 9.0,
        ..kit.clone()
    };
    assert_eq!(
        wild.settings(Some(2)).velocity_amount,
        GROOVE_VELOCITY_AMOUNT_MAX,
        "sanitized"
    );

    assert!(KitGroove::from_rack(&RackGrooveSettings::default(), &pool).is_none());
    let dangling = RackGrooveSettings {
        active: Some(99),
        ..settings
    };
    assert!(KitGroove::from_rack(&dangling, &pool).is_none());
    assert_eq!(
        legacy_builtin_groove("mpc-8-54"),
        Some(mpc_swing_groove(54, 0.5))
    );
    assert_eq!(legacy_builtin_groove("mpc-16-99"), None);
    assert_eq!(legacy_builtin_groove("swing"), None);
}

/// Cross-kit application: a pool groove extracted on one rack, applied to a
/// rack with a different pad set, plays each target pad through the source
/// row of the SAME pad note and every other pad through the shared row.
#[test]
fn a_copied_groove_maps_target_pads_by_note_and_falls_back_to_shared() {
    let groove = two_slot_groove(3, "Take", 0.25, &[36, 42]);
    // Target: hat (42) and snare (38); no kick.
    let mut target = rack_with_pads(&[42, 38]);
    assert_eq!(
        target.groove_row_mapping(&groove),
        vec![GrooveRowChoice::Pad, GrooveRowChoice::Shared]
    );
    let mut pool = Vec::new();
    let id = import_groove(&mut pool, &groove).expect("groove lands in the pool");
    target.groove.active = Some(id);
    let members = vec![5usize, 2];
    let table = track_groove_snapshots([(members.as_slice(), &target)], &pool, 6);
    assert_eq!(
        *table[5].as_ref().expect("hat grooved").row,
        *groove.pad_row(42).unwrap(),
        "the hat pad plays the source's hat row"
    );
    assert_eq!(
        *table[2].as_ref().expect("snare grooved").row,
        groove.shared_row,
        "the snare has no source row: shared"
    );
}

// ---------------------------------------------------------------------------
// Velocity + random amounts (eseq-groove.5)
// ---------------------------------------------------------------------------

/// A groove whose slots carry accents and spreads, for the velocity and
/// random amounts.
fn accented_groove(
    offsets: &[f32],
    velocity_scales: &[f32],
    spreads: &[f32],
) -> TrackGrooveSnapshot {
    let mut groove = track_groove(offsets.len() as f64 * 0.25, 0.25, offsets);
    let row = std::sync::Arc::make_mut(&mut groove.row);
    for (index, slot) in row.slots.iter_mut().enumerate() {
        slot.velocity_scale = velocity_scales[index];
        slot.spread = spreads[index];
    }
    groove
}

/// Velocity amount 0 (the default) leaves every velocity bit for bit, even
/// on a heavily accented row, and even an out-of-range source velocity.
#[test]
fn velocity_amount_zero_leaves_velocity_unchanged() {
    let groove = accented_groove(&[0.0, 0.2], &[1.8, 0.2], &[0.0, 0.0]);
    assert_eq!(groove.velocity_amount, 0.0);
    for beats in [0.0, 0.125, 0.25, 0.3, 7.75, -3.0] {
        assert_eq!(groove.velocity_scale(beats), 1.0);
        for velocity in [0.0_f32, 0.37, 1.0, 1.4, -0.2] {
            assert_eq!(
                groove.apply_velocity(velocity, beats).to_bits(),
                velocity.to_bits(),
                "beat {beats} velocity {velocity}"
            );
        }
    }
    // A neutral row at full amount is also a no-op.
    let mut neutral = accented_groove(&[0.0, 0.2], &[1.0, 1.0], &[0.0, 0.0]);
    neutral.velocity_amount = 1.5;
    assert_eq!(
        neutral.apply_velocity(0.37, 0.25).to_bits(),
        0.37_f32.to_bits()
    );
}

/// `velocity_amount` lerps the slot accent into the resolved velocity:
/// `lerp(1, lerp(scale[k], scale[k+1], t), amount)`, interpolating between
/// slots and clamping the product to the Velocity param's range.
#[test]
fn velocity_amount_lerps_slot_accent_and_clamps() {
    let mut groove = accented_groove(&[0.0, 0.0, 0.0, 0.0], &[1.5, 0.5, 1.0, 0.25], &[0.0; 4]);
    groove.velocity_amount = 1.0;
    let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
    assert!(close(groove.velocity_scale(0.0), 1.5));
    assert!(close(groove.velocity_scale(0.25), 0.5));
    assert!(close(groove.velocity_scale(0.75), 0.25));
    // A 32nd between slots 0 and 1: halfway between the accents.
    assert!(close(groove.velocity_scale(0.125), 1.0));
    // Period wrap: bar 3 reads the same accents.
    assert!(close(groove.velocity_scale(8.25), 0.5));
    // Between the last slot and the wrap to slot 0.
    assert!(close(groove.velocity_scale(0.875), (0.25 + 1.5) / 2.0));

    assert!(close(groove.apply_velocity(0.6, 0.25), 0.3));
    assert!(close(groove.apply_velocity(0.6, 0.0), 0.9));
    assert_eq!(
        groove.apply_velocity(0.8, 0.0),
        1.0,
        "0.8 * 1.5 clamps to max"
    );

    groove.velocity_amount = 0.5;
    assert!(close(groove.velocity_scale(0.0), 1.25));
    assert!(close(groove.velocity_scale(0.25), 0.75));
    assert!(close(groove.apply_velocity(0.4, 0.25), 0.3));

    // Over-amount can push a scale below zero: floored, never negative.
    let mut deep = accented_groove(&[0.0, 0.0], &[0.1, 1.0], &[0.0; 2]);
    deep.velocity_amount = 1.5;
    assert_eq!(deep.velocity_scale(0.0), 0.0);
    assert_eq!(deep.apply_velocity(0.9, 0.0), 0.0);
    // Degenerate beats never touch velocity.
    assert_eq!(deep.velocity_scale(f64::NAN), 1.0);
    assert_eq!(deep.apply_velocity(0.9, f64::INFINITY), 0.9);
}

/// The noise is a pure function of (absolute slot, pad note), in [-1, 1),
/// and decorrelated across slots and pads.
#[test]
fn groove_hash_noise_is_deterministic_bounded_and_varied() {
    let mut sum = 0.0;
    let mut distinct = std::collections::BTreeSet::new();
    for slot in -64..512_i64 {
        let value = groove_hash_noise(slot, 42);
        assert_eq!(value.to_bits(), groove_hash_noise(slot, 42).to_bits());
        assert!((-1.0..1.0).contains(&value), "{value}");
        sum += value;
        distinct.insert(value.to_bits());
    }
    assert_eq!(distinct.len(), 576, "every slot draws its own value");
    assert!(
        (sum / 576.0).abs() < 0.15,
        "roughly centered: mean {}",
        sum / 576.0
    );
    assert_ne!(groove_hash_noise(17, 36), groove_hash_noise(17, 42));
    assert_ne!(groove_hash_noise(17, 42), groove_hash_noise(18, 42));
}

/// Random adds `random * spread[k] * noise(absolute slot, pad)` (in slots,
/// then scaled by timing): the same boundary always jitters the same way,
/// the same slot in another bar jitters differently, and zero random or zero
/// spread is exactly the un-randomized offset.
#[test]
fn random_amount_jitters_by_spread_reproducibly_and_varies_bar_to_bar() {
    let offsets = [0.3_f32, 0.3, 0.3, 0.3];
    let spreads = [0.1_f32, 0.1, 0.0, 0.1];
    let plain = accented_groove(&offsets, &[1.0; 4], &spreads);
    let mut random = plain.clone();
    random.random_amount = 1.0;
    random.pad_note = 42;

    // Zero random = today's offset, bit for bit, whatever the spread.
    for beats in [0.0, 0.25, 0.625, 3.5] {
        assert_eq!(plain.offset_beats(beats).to_bits(), {
            let mut no_spread = plain.clone();
            std::sync::Arc::make_mut(&mut no_spread.row)
                .slots
                .iter_mut()
                .for_each(|slot| slot.spread = 0.0);
            no_spread.offset_beats(beats).to_bits()
        });
    }

    let mut per_bar = Vec::new();
    for bar in 0..8 {
        let beats = bar as f64 * 1.0 + 0.25; // slot 1 of every bar
        let value = random.offset_beats(beats);
        // Reproducible: same boundary, same value.
        assert_eq!(value.to_bits(), random.offset_beats(beats).to_bits());
        assert_eq!(
            value.to_bits(),
            random.clone().offset_beats(beats).to_bits()
        );
        // Bounded by random * spread (0.1 slot = 0.025 beats) around 0.3.
        let base = 0.3 * 0.25;
        assert!(
            (value - base).abs() <= 0.1 * 0.25 + 1e-9,
            "bar {bar}: {value}"
        );
        // Matches the documented formula.
        let expected = (0.3 + 0.1 * groove_hash_noise(bar * 4 + 1, 42)) * 0.25;
        assert!(
            (value - expected).abs() < 1e-7,
            "bar {bar}: {value} vs {expected}"
        );
        per_bar.push(value.to_bits());
    }
    let distinct: std::collections::BTreeSet<_> = per_bar.iter().collect();
    assert_eq!(
        distinct.len(),
        per_bar.len(),
        "each bar jitters differently"
    );

    // A slot with no spread never jitters.
    for bar in 0..4 {
        let beats = bar as f64 + 0.5;
        assert_eq!(
            random.offset_beats(beats).to_bits(),
            plain.offset_beats(beats).to_bits()
        );
    }
    // Another pad on the same slot draws other noise.
    let mut other_pad = random.clone();
    other_pad.pad_note = 36;
    assert_ne!(other_pad.offset_beats(0.25), random.offset_beats(0.25));
    // Half random halves the jitter; timing scales it with the offset.
    let mut half = random.clone();
    half.random_amount = 0.5;
    let jitter = |g: &TrackGrooveSnapshot| g.offset_beats(0.25) - plain.offset_beats(0.25);
    assert!((jitter(&half) - jitter(&random) * 0.5).abs() < 1e-7);
    let mut slow = random.clone();
    slow.timing_amount = 0.5;
    assert!((slow.offset_beats(0.25) - random.offset_beats(0.25) * 0.5).abs() < 1e-7);
    // A boundary a hair off the slot (float noise from another source) reads
    // the same slot AND the same seed.
    assert_eq!(
        random.offset_beats(0.25 - 1e-9).to_bits(),
        random.offset_beats(0.25).to_bits()
    );
}

#[test]
fn picker_keys_round_trip_and_reject_unknown_grooves() {
    for choice in [
        GrooveChoice::Off,
        GrooveChoice::Pool(7),
        GrooveChoice::Library {
            tier: GrooveLibraryTier::Factory,
            stem: "mpc-swing-58-16th".to_string(),
        },
        GrooveChoice::Library {
            tier: GrooveLibraryTier::User,
            stem: "Dilla-take".to_string(),
        },
    ] {
        assert_eq!(
            GrooveChoice::from_picker_key(&choice.picker_key()),
            Ok(choice)
        );
    }
    assert_eq!(GrooveChoice::Pool(7).picker_key(), "pool:7");
    assert_eq!(GrooveChoice::from_picker_key(""), Ok(GrooveChoice::Off));
    for bad in [
        "pool:x",
        "swing",
        "rack:1",
        "builtin:mpc-16-58",
        "user:",
        "user:../x",
        "factory:a/b",
    ] {
        assert!(GrooveChoice::from_picker_key(bad).is_err(), "{bad}");
    }
}

// ---------------------------------------------------------------------------
// Groove library (eseq-groove.9)
// ---------------------------------------------------------------------------

fn write(path: &std::path::Path, groove: &ProjectGroove) {
    library::write_groove_file(path, groove).expect("write groove file");
}

/// A `.groove` file is a versioned `ProjectGroove` without `id`: it round
/// trips everything but the id, is named by its stem when it sets no name,
/// and a newer generation or malformed grid is refused.
#[test]
fn groove_file_round_trips_without_id_and_refuses_newer_or_malformed_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let groove = two_slot_groove(42, "Dilla Pocket", 0.2, &[36, 42]);
    let path = dir.path().join("dilla.groove");
    write(&path, &groove);
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["groove_version"], GROOVE_FILE_VERSION);
    assert!(json.get("id").is_none(), "ids are pool-local: {json}");
    let read = library::read_groove_file(&path).expect("read back");
    assert_eq!(read.id, 0);
    assert!(read.same_feel(&groove), "everything but the id round-trips");

    // No name: the file stem names it.
    let mut unnamed = json.clone();
    unnamed.as_object_mut().unwrap().remove("name");
    unnamed.as_object_mut().unwrap().remove("groove_version");
    let stem_path = dir.path().join("Stem-Name.groove");
    std::fs::write(&stem_path, unnamed.to_string()).unwrap();
    assert_eq!(
        library::read_groove_file(&stem_path).unwrap().name,
        "Stem-Name"
    );

    let mut newer = json.clone();
    newer["groove_version"] = serde_json::json!(GROOVE_FILE_VERSION + 1);
    std::fs::write(&path, newer.to_string()).unwrap();
    assert!(library::read_groove_file(&path).is_err());
    let mut broken = json;
    broken["shared_row"]["slots"] = serde_json::json!([]);
    std::fs::write(&path, broken.to_string()).unwrap();
    assert!(library::read_groove_file(&path).is_err());
    let mut malformed = groove.clone();
    malformed.resolution_beats = 0.3;
    assert!(library::write_groove_file(&path, &malformed).is_err());
}

/// The library lists factory files first, then user files, each sorted,
/// skipping anything that is not a valid `.groove`; a missing tier is empty.
/// Save never overwrites, rename moves the file and its name, delete removes
/// it, and loading goes by tier + stem.
#[test]
fn library_lists_factory_then_user_and_edits_only_user_files() {
    let factory = tempfile::tempdir().expect("factory dir");
    let user_root = tempfile::tempdir().expect("user dir");
    let user = user_root.path().join("grooves");
    write(
        &factory.path().join("b-swing.groove"),
        &mpc_swing_groove(58, 0.25),
    );
    write(
        &factory.path().join("a-swing.groove"),
        &mpc_swing_groove(66, 0.5),
    );
    std::fs::write(factory.path().join("junk.groove"), "not json").unwrap();
    std::fs::write(factory.path().join("notes.txt"), "ignored").unwrap();
    assert!(
        library::list_groove_library_in(factory.path(), &user)
            .iter()
            .all(|entry| entry.tier == GrooveLibraryTier::Factory),
        "a missing user tier is empty"
    );

    let take = two_slot_groove(3, "Dilla Take", 0.2, &[36]);
    let saved = library::save_groove_to_library_in(&user, "Dilla Take", &take).expect("save");
    assert_eq!(saved.file_name().unwrap(), "Dilla-Take.groove");
    let again = library::save_groove_to_library_in(&user, "Dilla Take", &take).expect("save again");
    assert_eq!(
        again.file_name().unwrap(),
        "Dilla-Take-2.groove",
        "a save never overwrites"
    );
    assert!(library::save_groove_to_library_in(&user, "  ", &take).is_err());

    let listed = library::list_groove_library_in(factory.path(), &user);
    let names = listed
        .iter()
        .map(|entry| (entry.tier, entry.stem.as_str(), entry.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            (GrooveLibraryTier::Factory, "a-swing", "MPC 8 Swing 66%"),
            (GrooveLibraryTier::Factory, "b-swing", "MPC 16 Swing 58%"),
            (GrooveLibraryTier::User, "Dilla-Take", "Dilla Take"),
            (GrooveLibraryTier::User, "Dilla-Take-2", "Dilla Take"),
        ],
        "factory first, then user; junk skipped"
    );
    assert_eq!(listed[2].choice().picker_key(), "user:Dilla-Take");
    let loaded = library::load_library_groove_in(
        factory.path(),
        &user,
        GrooveLibraryTier::User,
        "Dilla-Take",
    )
    .expect("load by stem");
    assert!(loaded.same_feel(&take));
    assert!(library::load_library_groove_in(
        factory.path(),
        &user,
        GrooveLibraryTier::Factory,
        "../grooves/Dilla-Take",
    )
    .is_err());

    let renamed =
        library::rename_library_groove_in(&user, "Dilla-Take-2", "Madlib").expect("rename");
    assert_eq!(renamed.file_name().unwrap(), "Madlib.groove");
    assert!(!user.join("Dilla-Take-2.groove").exists());
    assert_eq!(library::read_groove_file(&renamed).unwrap().name, "Madlib");
    library::delete_library_groove_in(&user, "Madlib").expect("delete");
    let stems = library::list_groove_library_in(factory.path(), &user)
        .into_iter()
        .map(|entry| entry.stem)
        .collect::<Vec<_>>();
    assert_eq!(stems, vec!["a-swing", "b-swing", "Dilla-Take"]);
    assert!(library::delete_library_groove_in(&user, "Madlib").is_err());
}

// ---------------------------------------------------------------------------
// Record unwind (eseq-groove.6 / eseq-k0v8)
// ---------------------------------------------------------------------------

/// Straight 16th geometry for `n` steps: (boundaries, step ends, cycle).
fn sixteenth_geometry(n: usize) -> (Vec<f64>, Vec<f64>, f64) {
    let boundaries = (0..n).map(|s| s as f64 * SIXTEENTH).collect::<Vec<_>>();
    let ends = (0..n)
        .map(|s| (s + 1) as f64 * SIXTEENTH)
        .collect::<Vec<_>>();
    (boundaries, ends, n as f64 * SIXTEENTH)
}

fn assert_unwound(got: Option<UnwoundPosition>, step: usize, phase: f64, what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what}: expected a position"));
    assert_eq!(got.step, step, "{what}: step");
    assert!(
        (got.phase - phase).abs() < 1.0e-6,
        "{what}: phase {} vs {phase}",
        got.phase
    );
}

/// Swing at 75/16th: an odd step's hit heard on its swung position (or past
/// it) stores the straight phase; even steps are untouched; the downbeat
/// right after a swung step reads as the downbeat, not the swung step's tail.
#[test]
fn unwind_swing_stores_the_straight_phase_of_the_heard_step() {
    let (boundaries, ends, cycle) = sixteenth_geometry(16);
    let swing = |step: usize, _base: f64| {
        swing_shift_beats(75.0, SwingResolution::Sixteenth, boundaries[step])
    };
    assert_eq!(swing(1, 0.0), 0.125);
    assert_eq!(swing(2, 0.0), 0.0);
    let unwind = |heard: f64| unwind_step_feel(heard, cycle, &boundaries, &ends, swing);
    assert_unwound(unwind(0.25 + 0.125), 1, 0.0, "on the swung 16th");
    assert_unwound(unwind(0.25 + 0.125 + 0.05), 1, 0.2, "past the swung 16th");
    assert_unwound(unwind(0.5), 2, 0.0, "downbeat after a swung step");
    assert_unwound(unwind(0.5 + 0.05), 2, 0.2, "inside an even step");
    // The gap a late step opens: heard before its swung position reads as
    // slightly early for it.
    assert_unwound(unwind(0.25 + 0.05), 1, 0.0, "early for the swung 16th");
}

/// A groove with late and early slots: the stored phase is the heard beat
/// minus the pocket of the step it lands on, including an early first step
/// heard at the end of the previous cycle.
#[test]
fn unwind_groove_pocket_late_and_early_slots_and_cycle_wrap() {
    let (boundaries, ends, cycle) = sixteenth_geometry(4);
    let groove = track_groove(1.0, SIXTEENTH, &[-0.2, 0.3, 0.0, -0.1]);
    let cycle_start = 8.0; // transport beat of the heard cycle
    let pocket =
        |step: usize, base: f64| groove.pocket_offset_beats(cycle_start + base + boundaries[step]);
    let unwind = |heard: f64| unwind_step_feel(heard, cycle, &boundaries, &ends, pocket);
    for (step, phase) in [(1, 0.0), (1, 0.4), (2, 0.0), (2, 0.5), (3, 0.0), (3, 0.3)] {
        let heard = boundaries[step]
            + phase * SIXTEENTH
            + groove.pocket_offset_beats(cycle_start + boundaries[step]);
        assert_unwound(
            unwind(heard),
            step,
            phase,
            &format!("step {step} phase {phase}"),
        );
    }
    // Step 0 is early by 0.2 slot: heard before the cycle's end.
    assert_unwound(
        unwind(cycle - 0.2 * SIXTEENTH),
        0,
        0.0,
        "early step 0 wraps forward",
    );
    assert_unwound(unwind(0.1 * SIXTEENTH), 0, 0.3, "inside early step 0");
}

/// No feel: the unwind is the plain lookup, and a straight Sync gap stays
/// unresolved exactly as before.
#[test]
fn unwind_without_feel_matches_the_plain_lookup_and_keeps_sync_gaps() {
    // Two steps with a Sync wait between them: [0, 0.25) then [0.5, 0.75).
    let boundaries = [0.0, 0.5];
    let ends = [0.25, 0.75];
    let unwind = |heard: f64| unwind_step_feel(heard, 1.0, &boundaries, &ends, |_, _| 0.0);
    assert_unwound(unwind(0.1), 0, 0.4, "plain");
    assert_unwound(unwind(0.6), 1, 0.4, "plain second step");
    assert_eq!(unwind(0.3), None, "a Sync wait is no step");
}

/// Recording unwinds the deterministic pocket: Random's jitter is not part
/// of it, so a recorded hit is not printed with one bar's noise.
#[test]
fn pocket_offset_excludes_random_jitter() {
    let mut groove = accented_groove(&[0.1, 0.3], &[1.0, 1.0], &[0.4, 0.4]);
    let beat = 0.25;
    let pocket = groove.pocket_offset_beats(beat);
    assert!((pocket - 0.3 * 0.25).abs() < 1.0e-7);
    groove.random_amount = 1.0;
    assert_eq!(groove.pocket_offset_beats(beat), pocket);
    assert_ne!(
        groove.offset_beats(beat),
        pocket,
        "the played offset does jitter"
    );
}

// --- pad roles (eseq-groove.10) ------------------------------------------------

/// A groove extracted from a standard-layout kit: kick (pad 0), snare (pad 2)
/// and closed hat (pad 6), each pad late by its own amount, so every pad row
/// is distinguishable. Roles come from the pads' effective roles, the way
/// `App::extract_rack_groove_recorded` passes them.
fn standard_kit_groove() -> (crate::project::ProjectRackConfig, ProjectGroove) {
    use crate::project::{ProjectRackConfig, ProjectRackPad};
    let kit = ProjectRackConfig {
        pads: vec![
            ProjectRackPad::new(0, 0),
            ProjectRackPad::new(2, 1),
            ProjectRackPad::new(6, 2),
        ],
        groove: RackGrooveSettings {
            active: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let late = |beats: &[f64], by: f64| {
        beats
            .iter()
            .map(|&beat| hit(beat + by * SIXTEENTH, 0.8))
            .collect::<Vec<_>>()
    };
    let sources = [
        (0, late(&[0.0, 2.0], 0.05)),
        (2, late(&[1.0, 3.0], 0.2)),
        (6, late(&[0.5, 1.5, 2.5, 3.5], 0.35)),
    ]
    .into_iter()
    .map(|(pad_note, hits)| GroovePadSource {
        pad_note,
        role: kit
            .pads
            .iter()
            .find(|pad| pad.pad_note == pad_note)
            .and_then(|pad| pad.effective_role()),
        hits,
    })
    .collect::<Vec<_>>();
    let groove =
        extract_groove(1, &options(GROOVE_PERIOD_ONE_BAR, SIXTEENTH), &sources).expect("extract");
    (kit, groove)
}

#[test]
fn extraction_records_each_source_pads_role_on_its_row() {
    let (_, groove) = standard_kit_groove();
    let roles = groove
        .pad_rows
        .iter()
        .map(|row| (row.pad_note, row.role))
        .collect::<Vec<_>>();
    assert_eq!(
        roles,
        vec![
            (0, Some(PadRole::Kick)),
            (2, Some(PadRole::Snare)),
            (6, Some(PadRole::ClosedHat)),
        ]
    );
    // A pad without a role records none, and its row writes no role key.
    let groove = extract_groove(
        2,
        &options(GROOVE_PERIOD_ONE_BAR, SIXTEENTH),
        &[pad(40, vec![hit(0.0, 1.0)])],
    )
    .unwrap();
    assert_eq!(groove.pad_rows[0].role, None);
    let json = serde_json::to_string(&groove).unwrap();
    assert!(!json.contains("\"role\""), "{json}");
    // A groove saved before roles loads with unrecorded roles.
    let (_, tagged) = standard_kit_groove();
    let json = serde_json::to_string(&tagged).unwrap();
    assert!(json.contains("\"role\":\"closed-hat\""), "{json}");
    let old = json
        .replace(",\"role\":\"kick\"", "")
        .replace(",\"role\":\"snare\"", "")
        .replace(",\"role\":\"closed-hat\"", "");
    let old: ProjectGroove = serde_json::from_str(&old).expect("pre-role groove loads");
    assert!(old.pad_rows.iter().all(|row| row.role.is_none()));
    assert!(old.is_well_formed());
}

/// `row_for_pad` order: same pad note when the roles are compatible (equal,
/// or either unknown), then the lowest-note row with the pad's role, then
/// the shared row.
#[test]
fn row_lookup_prefers_same_note_then_same_role_then_shared() {
    let mut groove = mpc_swing_groove(50, 0.25);
    let row = |pad_note: i32, role: Option<PadRole>, offset: f32| GroovePadRow {
        pad_note,
        role,
        row: row_of(&[offset, 0.0]),
    };
    groove.pad_rows = vec![
        row(0, Some(PadRole::Kick), 0.01),
        row(4, Some(PadRole::Snare), 0.04),
        row(2, Some(PadRole::Snare), 0.02),
        row(30, None, 0.30),
    ];
    let pick = |pad_note: i32, role: Option<PadRole>| {
        groove
            .resolve_pad_row(pad_note, role)
            .map(|row| row.pad_note)
    };
    // 1. Same note, same role (the same kit).
    assert_eq!(pick(0, Some(PadRole::Kick)), Some(0));
    // 1. Same note wins over the lower-note row with the same role.
    assert_eq!(pick(4, Some(PadRole::Snare)), Some(4));
    // 1. Unknown on either side is compatible.
    assert_eq!(pick(0, None), Some(0));
    assert_eq!(pick(30, Some(PadRole::Ride)), Some(30));
    // 2. The note's row is another drum: the row with the pad's role.
    assert_eq!(pick(2, Some(PadRole::Kick)), Some(0));
    // 2. No row at this note: the lowest-note row with the role.
    assert_eq!(pick(9, Some(PadRole::Snare)), Some(2));
    // 3. Nothing fits: the shared row.
    assert_eq!(pick(2, Some(PadRole::Clap)), None);
    assert_eq!(pick(9, Some(PadRole::Clap)), None);
    assert_eq!(pick(9, None), None);
    assert_eq!(groove.row_for_pad(9, None), &groove.shared_row);
    assert_eq!(
        groove.row_for_pad(9, Some(PadRole::Snare)),
        &groove.pad_rows[2].row
    );
}

/// Cross-kit: a groove extracted on a standard-layout kit, applied to a kit
/// laid out differently. The snare row lands on the snare wherever it sits,
/// a pad on the source's snare NOTE that is a hat takes the hat row, and a
/// pad on the source hat's note that is a perc (no perc row) plays shared.
#[test]
fn cross_kit_rows_follow_roles_not_notes() {
    use crate::project::{ProjectRackConfig, ProjectRackPad};
    let (_, groove) = standard_kit_groove();
    let tagged = |pad_note: i32, member: usize, role: PadRole| ProjectRackPad {
        pad_note,
        member,
        role: Some(role),
    };
    let other_kit = ProjectRackConfig {
        pads: vec![
            tagged(12, 0, PadRole::Kick),
            tagged(20, 1, PadRole::Snare),
            tagged(2, 2, PadRole::ClosedHat),
            tagged(6, 3, PadRole::Perc),
        ],
        groove: RackGrooveSettings {
            active: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let members = [10usize, 11, 12, 13];
    let pool = vec![groove.clone()];
    let table = track_groove_snapshots([(&members[..], &other_kit)], &pool, 14);
    let row_of_track = |track: usize| (*table[track].as_ref().expect("grooved").row).clone();
    let source = |note: i32| groove.pad_row(note).unwrap().clone();
    assert_eq!(row_of_track(10), source(0), "kick at pad 12: the kick row");
    assert_eq!(
        row_of_track(11),
        source(2),
        "snare at pad 20: the snare row"
    );
    assert_ne!(source(2), source(6));
    assert_eq!(
        row_of_track(12),
        source(6),
        "a hat on the source snare's note takes the hat row, not the snare row"
    );
    assert_eq!(
        row_of_track(13),
        groove.shared_row,
        "a perc on the source hat's note: no perc row, the shared row"
    );
    assert_eq!(
        other_kit.groove_row_mapping(&groove),
        vec![
            GrooveRowChoice::Pad,
            GrooveRowChoice::Pad,
            GrooveRowChoice::Pad,
            GrooveRowChoice::Shared
        ]
    );
}

/// Same kit: every pad still plays its own-note row, exactly as a groove
/// without recorded roles resolves (the pre-role behavior); and a role-less
/// groove on a re-laid-out kit keeps the old by-note mapping.
#[test]
fn same_kit_and_role_less_grooves_resolve_by_pad_note_as_before() {
    let (kit, groove) = standard_kit_groove();
    let mut role_less = groove.clone();
    for row in &mut role_less.pad_rows {
        row.role = None;
    }
    let members = [0usize, 1, 2];
    let with_roles = track_groove_snapshots([(&members[..], &kit)], &[groove.clone()], 3);
    let without = track_groove_snapshots([(&members[..], &kit)], &[role_less.clone()], 3);
    assert_eq!(with_roles, without);
    for (track, note) in [(0usize, 0), (1, 2), (2, 6)] {
        assert_eq!(
            *with_roles[track].as_ref().unwrap().row,
            *groove.pad_row(note).unwrap(),
            "pad {note} plays its own row"
        );
    }
    // Explicitly tagging a same-kit pad with the role it already infers
    // changes nothing.
    let mut tagged = kit.clone();
    tagged.pads[1].role = Some(PadRole::Snare);
    assert_eq!(
        track_groove_snapshots([(&members[..], &tagged)], &[groove.clone()], 3),
        with_roles
    );
    // A role-less groove on a kit whose pad 2 is a hat: by note, as before.
    let mut relaid = kit.clone();
    relaid.pads[1].role = Some(PadRole::ClosedHat);
    let table = track_groove_snapshots([(&members[..], &relaid)], &[role_less.clone()], 3);
    assert_eq!(
        *table[1].as_ref().unwrap().row,
        *role_less.pad_row(2).unwrap()
    );
}
