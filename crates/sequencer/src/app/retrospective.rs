//! Transport-independent history of live note gestures. Owned by the control
//! thread; neither the audio callback nor the scheduler touches this buffer.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use crate::audio::MAX_VOICES;
use crate::sequencer::{SceneId, StepParam, Timebase, TrackId, MAX_STEPS};
use super::App;

pub const CAPTURE_WINDOW: Duration = Duration::from_secs(30);
const MAX_CAPTURE_NOTES: usize = 8192;
const MIN_CAPTURE_BPM: f64 = 70.0;

/// Interpret slow phrases in double time without changing their time range.
/// Shared by the crop UI, audition and import so their bar counts agree.
pub fn capture_bar_count(duration: f64, requested_bars: usize) -> Result<usize, String> {
    if !duration.is_finite() || duration <= 0.0 {
        return Err("Choose a non-empty crop inside the captured time range".into());
    }
    if requested_bars == 0 || requested_bars > MAX_STEPS / 16 {
        return Err(format!("Choose between 1 and {} bars", MAX_STEPS / 16));
    }
    let mut bars = requested_bars;
    while (240.0 * bars as f64 / duration).round() < MIN_CAPTURE_BPM {
        bars *= 2;
        if bars > MAX_STEPS / 16 {
            return Err("Shorten the crop to reach at least 70 BPM within the pattern length limit".into());
        }
    }
    Ok(bars)
}

/// A suggested crop: the phrase's first onset, a whole-number tempo whose
/// power-of-two bar count matches the repeat the player looped, and that
/// bar count. The crop end follows as `start + 240 * bars / bpm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureGuess {
    pub start: f64,
    pub bpm: u32,
    pub bars: usize,
}

/// Onsets closer than this collapse into one hit (chords, flams).
const ONSET_MERGE: f64 = 0.03;
/// Tempo window, one octave wide so the choice of octave is unique. Placed
/// low so hip-hop tempos stay put; faster grooves read as half time, which
/// keeps the loop's duration.
const GUESS_MIN_BPM: f64 = 70.0;
const GUESS_MAX_BPM: f64 = 140.0;

/// How strongly onsets line up on a grid of `spacing` seconds, 0..1, and
/// where (seconds, modulo `spacing`) that grid's lines fall.
fn grid_fit(onsets: &[f64], spacing: f64) -> (f64, f64) {
    let (mut cos, mut sin) = (0.0, 0.0);
    for &time in onsets {
        let angle = std::f64::consts::TAU * time / spacing;
        cos += angle.cos();
        sin += angle.sin();
    }
    let phase = sin.atan2(cos) / std::f64::consts::TAU * spacing;
    ((cos * cos + sin * sin).sqrt() / onsets.len() as f64, phase)
}

/// Pulse strength of a tempo: every hit of every sound should land on its
/// sixteenth, eighth and quarter grids. Wrong tempos may fit one level by
/// accident but not all three, and timing slop costs every candidate alike.
fn tempo_fit(onsets: &[f64], bpm: f64) -> f64 {
    let beat = 60.0 / bpm;
    [beat / 4.0, beat / 2.0, beat].iter().map(|&spacing| grid_fit(onsets, spacing).0).sum()
}

/// Guess where the looped phrase starts, its tempo and its length in bars.
///
/// 1. Isolated hits (a stray note seconds before the groove) are dropped by
///    splitting the onsets at long silences and keeping the densest phrase.
/// 2. The tempo is the pulse that all hits, across all sounds, fit best
///    (`tempo_fit`), searched within one octave.
/// 3. The start is the phrase's first hit, snapped onto the tempo's grid.
/// 4. The loop length is the shortest power-of-two bar count after which
///    each sound's quantized steps repeat about as well as at any longer one.
///
/// Returns `None` when the phrase has too few hits or no steady pulse.
pub fn detect_capture_loop(notes: &[CapturedNote]) -> Option<CaptureGuess> {
    let mut hits: Vec<(f64, (TrackId, i32))> = notes.iter()
        .filter(|note| note.start.is_finite())
        .map(|note| (note.start, (note.track, (note.transpose * 100.0).round() as i32)))
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut all: Vec<f64> = Vec::new();
    for &(time, _) in &hits {
        if all.last().is_none_or(|last| time - last > ONSET_MERGE) {
            all.push(time);
        }
    }
    if all.len() < 4 { return None; }

    // Phrase: split at silences much longer than the typical gap.
    let mut gaps: Vec<f64> = all.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    let split = (4.0 * gaps[gaps.len() / 2]).max(2.5);
    let mut phrase = (0, 0);
    let mut first = 0;
    for index in 1..=all.len() {
        if index == all.len() || all[index] - all[index - 1] > split {
            // Ties keep the later phrase: it is what was just played.
            if index - first >= phrase.1 - phrase.0 { phrase = (first, index); }
            first = index;
        }
    }
    let onsets = &all[phrase.0..phrase.1];
    if onsets.len() < 4 { return None; }
    let (from, to) = (onsets[0], onsets[onsets.len() - 1]);

    // Coarse scan in 0.1 BPM, then refine around the winner.
    let mut best = (0.0, f64::NEG_INFINITY);
    for tenth in (GUESS_MIN_BPM * 10.0) as u32..(GUESS_MAX_BPM * 10.0) as u32 {
        let bpm = tenth as f64 / 10.0;
        let fit = tempo_fit(onsets, bpm);
        if fit > best.1 { best = (bpm, fit); }
    }
    for hundredth in -10..=10 {
        let bpm = best.0 + hundredth as f64 / 100.0;
        let fit = tempo_fit(onsets, bpm);
        if fit > best.1 { best = (bpm, fit); }
    }
    let (exact_bpm, fit) = best;
    let sixteenth = 15.0 / exact_bpm;
    let (sixteenth_fit, phase) = grid_fit(onsets, sixteenth);
    // Steady: the three grid levels agree, or every hit sits tightly on the
    // sixteenth grid (busy off-beat playing leaves the coarser levels empty).
    // Calibrated against random timing (fit ~0.84, z ~1.6).
    if fit < 0.95 && sixteenth_fit * (onsets.len() as f64).sqrt() < 3.0 { return None; }
    let bpm = exact_bpm.round().clamp(GUESS_MIN_BPM, GUESS_MAX_BPM);

    // Start on the grid line nearest the first hit, not on its slop.
    let start = (phase + ((from - phase) / sixteenth).round() * sixteenth).max(0.0);

    // Loop length. Quantize every hit to the grid, which absorbs loose
    // timing, then ask how many of each sound's steps recur 1, 2, 4 or 8
    // bars later. Take the shortest length about as good as the best.
    let mut steps = std::collections::BTreeSet::<((TrackId, i32), i64)>::new();
    for &(time, lane) in &hits {
        if time >= from - ONSET_MERGE && time <= to + ONSET_MERGE {
            steps.insert((lane, ((time - start) / sixteenth).round() as i64));
        }
    }
    let last = steps.iter().map(|&(_, step)| step).max().unwrap_or(0);
    let repeats: Vec<(usize, f64)> = [1usize, 2, 4, 8].into_iter().filter_map(|bars| {
        let lag = 16 * bars as i64;
        let eligible: Vec<_> = steps.iter().filter(|&&(_, step)| step + lag <= last).collect();
        // A length needs enough hits that it can recur before the phrase ends.
        (eligible.len() >= 4 && eligible.len() * 4 >= steps.len()).then(|| {
            let matched = eligible.iter().filter(|&&&(lane, step)| steps.contains(&(lane, step + lag))).count();
            (bars, matched as f64 / eligible.len() as f64)
        })
    }).collect();
    let most = repeats.iter().map(|r| r.1).fold(0.0, f64::max);
    let bars = match repeats.iter().find(|r| most >= 0.5 && r.1 >= 0.9 * most) {
        Some(&(bars, _)) => bars,
        // Nothing repeats: cover the whole phrase.
        None => ((last + 1) as f64 / 16.0).ceil().clamp(1.0, 16.0) as usize,
    }.next_power_of_two().min(16);
    Some(CaptureGuess { start, bpm: bpm as u32, bars })
}

#[derive(Clone, Debug)]
struct LiveNote {
    generation: u64,
    track: TrackId,
    transpose: f32,
    velocity: f32,
    start: Instant,
    end: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct CapturedNote {
    pub track: TrackId,
    pub transpose: f32,
    pub velocity: f32,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug)]
pub struct CaptureDraft {
    pub notes: Vec<CapturedNote>,
    pub duration: f64,
    pub truncated: bool,
    pub scene: SceneId,
}

pub struct RetrospectiveCapture {
    notes: VecDeque<LiveNote>,
    started: Instant,
    last_eviction: Option<Instant>,
    pub draft: Option<CaptureDraft>,
}

impl Default for RetrospectiveCapture {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl RetrospectiveCapture {
    fn new(started: Instant) -> Self {
        Self {
            notes: VecDeque::with_capacity(MAX_CAPTURE_NOTES),
            started,
            last_eviction: None,
            draft: None,
        }
    }

    fn prune(&mut self, now: Instant) {
        while self.notes.front().is_some_and(|note| {
            now.saturating_duration_since(note.start) > CAPTURE_WINDOW
        }) {
            self.notes.pop_front();
        }
    }

    pub fn note_on(
        &mut self, generation: u64, track: TrackId, transpose: f32, velocity: f32,
        now: Instant,
    ) {
        if !transpose.is_finite() || !velocity.is_finite() || velocity <= 0.0 {
            return;
        }
        self.prune(now);
        if self.notes.len() == MAX_CAPTURE_NOTES {
            self.last_eviction = self.notes.pop_front().map(|note| note.start);
        }
        self.notes.push_back(LiveNote {
            generation, track, transpose, velocity: velocity.min(1.0), start: now, end: None,
        });
    }

    pub fn note_off(&mut self, generation: u64, now: Instant) {
        self.prune(now);
        for note in self.notes.iter_mut().rev() {
            if note.generation == generation && note.end.is_none() {
                note.end = Some(now.max(note.start));
            }
        }
    }

    /// On-screen pads are fixed-duration trigs, rather than held notes.
    pub fn trig(&mut self, track: TrackId, transpose: f32, now: Instant, duration: Duration) {
        if !transpose.is_finite() { return; }
        self.note_on(0, track, transpose, 1.0, now);
        if let Some(note) = self.notes.back_mut() {
            note.end = Some(now + duration);
        }
    }

    /// Freeze a copy. Subsequent playing, releases, and ring eviction cannot
    /// change what the user is cropping. Notes still held end at the snapshot.
    pub fn snapshot(&mut self, now: Instant, scene: SceneId) -> &CaptureDraft {
        self.prune(now);
        let start = now.checked_sub(CAPTURE_WINDOW).unwrap_or(self.started).max(self.started);
        let notes = self.notes.iter().filter(|note| note.start <= now).map(|note| {
            CapturedNote {
                track: note.track,
                transpose: note.transpose,
                velocity: note.velocity,
                start: note.start.saturating_duration_since(start).as_secs_f64(),
                end: note.end.unwrap_or(now).min(now).saturating_duration_since(start).as_secs_f64(),
            }
        }).collect();
        self.draft = Some(CaptureDraft {
            notes,
            duration: now.saturating_duration_since(start).as_secs_f64(),
            truncated: self.last_eviction.is_some_and(|time| time >= start),
            scene,
        });
        self.draft.as_ref().unwrap()
    }
}

/// A captured gesture expressed in the destination pattern's step units.
struct ImportedNote {
    transpose: f32,
    duration: f32,
    delay: f32,
    velocity: f32,
}

struct PreparedCapture {
    scenes: crate::sequencer::ProjectScenes,
    bpm: u32,
    steps: usize,
    note_count: usize,
    tracks: Vec<usize>,
}

impl App {
    /// Import fresh patterns into the scene reviewed by the user. All notes
    /// are validated before mutation, and history rolls back a failed install.
    /// The previous patterns remain in their pools and undo restores the cells.
    fn prepare_retrospective(&mut self, start: f64, end: f64, bars: usize) -> Result<PreparedCapture, String> {
        let draft = self.retrospective.draft.as_ref().ok_or("Open MIDI capture first")?;
        // The end may pass the snapshot: a tempo-driven crop can run into the
        // silence after the last note, which loops as a rest.
        if !start.is_finite() || !end.is_finite() || start < 0.0
            || end <= start || start >= draft.duration
        {
            return Err("Choose a non-empty crop inside the captured time range".into());
        }
        let bars = capture_bar_count(end - start, bars)?;
        if self.state.current_scene_id() != Some(draft.scene) {
            return Err("The scene changed. Reopen MIDI capture before importing".into());
        }
        if self.state.is_playing() || self.recording_history.is_some() {
            return Err("Stop playback before auditioning or sending the crop".into());
        }
        let bpm = (240.0 * bars as f64 / (end - start)).round();
        if !(MIN_CAPTURE_BPM..=999.0).contains(&bpm) {
            return Err("The loop tempo must be between 70 and 999 BPM. Adjust the crop or bar count".into());
        }
        let steps = bars * 16;
        let scale = steps as f64 / (end - start);
        // A quarter step of slop at each crop edge, so the loop reads as a
        // quantized cycle: an early first hit still lands on step one, and
        // the next repetition's early downbeat is left out of the last step.
        // A quarter keeps 16th triplets (a third of a step off the grid) out
        // of the slop.
        let slop = (end - start) / steps as f64 / 4.0;
        let mut lanes = BTreeMap::<usize, BTreeMap<usize, Vec<ImportedNote>>>::new();
        // Notes before the crop start go last, and only onto a track whose
        // step one is still empty, so a pickup or flam never doubles the
        // downbeat.
        for early in [false, true] { for note in &draft.notes {
            // A trig is selected by its onset, inside the crop window shifted
            // back by the slop. A note crossing the right crop edge is
            // shortened; cropping never invents a note-on at the left.
            if (note.start < start) != early
                || note.start < start - slop || note.start >= end - slop { continue; }
            let track = self.track_registry.index_of(note.track)
                .ok_or("A captured track was deleted. Reopen MIDI capture")?;
            if early && lanes.get(&track).is_some_and(|steps| steps.contains_key(&0)) { continue; }
            let position = ((note.start - start) * scale).max(0.0);
            let step = position.floor() as usize;
            let delay = (position - step as f64) as f32;
            let duration = ((note.end.min(end) - note.start).max(0.000001) * scale) as f32;
            if step >= steps || duration > StepParam::Duration.max() {
                return Err("A note exceeds the pattern duration limit. Choose fewer bars or a shorter note".into());
            }
            let notes = lanes.entry(track).or_default().entry(step).or_default();
            if notes.len() == MAX_VOICES {
                return Err(format!("Track {} has more than {MAX_VOICES} notes in one step. Choose more bars", track + 1));
            }
            // The existing pattern model has one velocity per step. Never
            // silently flatten independently played velocities on import.
            if notes.first().is_some_and(|first| first.velocity != note.velocity) {
                return Err(format!("Track {} has different velocities within one step. Choose more bars to separate those hits", track + 1));
            }
            notes.push(ImportedNote {
                transpose: note.transpose, duration, delay, velocity: note.velocity,
            });
        } }
        if lanes.is_empty() { return Err("The crop contains no trigs".into()); }
        let note_count = lanes.values().flat_map(|steps| steps.values()).map(Vec::len).sum();
        let tracks = lanes.keys().copied().collect();
        let mut scenes = self.capture_synchronized_scene_structure_state()?;
        for (track, notes_by_step) in lanes {
            let mut data = scenes.effective_track_pattern(track)
                .ok_or("A captured track has no current pattern")?;
            data.clear_step_content();
            data.track_params.timebase = Timebase::Sixteenth;
            data.track_params.num_steps = steps;
            data.track_params.swing = 50.0;
            for (step, notes) in notes_by_step {
                data.track_bits[step / 64] |= 1 << (step % 64);
                data.step_data[step][StepParam::Transpose.index()] = notes[0].transpose;
                data.step_data[step][StepParam::Duration.index()] = notes[0].duration;
                data.step_data[step][StepParam::Velocity.index()] = notes[0].velocity;
                for note in notes {
                    data.chord_snapshot.steps[step].push(note.transpose);
                    data.chord_snapshot.durations[step].push(note.duration);
                    data.chord_snapshot.delays[step].push(note.delay);
                }
            }
            let pool = &mut scenes.track_pools[track];
            let id = pool.insert(data);
            let sound = pool.refs(id).ok_or("Imported pattern has no sound")?;
            // Kit pads in a rack with clips play their clip's cell, not the
            // scene's; writing only the scene cell imported (and previewed)
            // nothing for them.
            let scene = scenes.current_scene;
            if scenes.rack_member_slot(track).is_none() {
                scenes.scenes[scene].cell_sounds[track] = sound;
            }
            if !scenes.set_composed_scene_cell(scene, track, id) {
                return Err(format!("Track {} has no pattern slot in this scene", track + 1));
            }
            scenes.track_overrides[track] = None;
        }
        Ok(PreparedCapture { scenes, bpm: bpm as u32, steps, note_count, tracks })
    }

    pub fn audition_retrospective(&mut self, start: f64, end: f64, bars: usize) -> Result<(), String> {
        let prepared = self.prepare_retrospective(start, end, bars)?;
        let mut tracks = Vec::new();
        for track in 0..self.tracks.len() {
            let mut data = prepared.scenes.effective_track_pattern(track)
                .ok_or("A track has no pattern for preview")?;
            // Only tracks that received a new pattern belong to the audition.
            if !prepared.tracks.contains(&track) {
                data.clear_step_content();
            }
            tracks.push(data);
        }
        let current = self.state.latest_scheduler_snapshot();
        let mut snapshot = crate::sequencer::SequencerSnapshot::capture_from_track_pattern_data(
            &self.state, &tracks, current.mod_connections.clone(), Vec::new(), Vec::new(),
            current.scene_slots.clone(), Default::default());
        snapshot.transport.bpm = prepared.bpm;
        self.state.note_audition.start(crate::scheduler::audition::AuditionLoop {
            snapshot, steps: prepared.steps,
        });
        Ok(())
    }

    /// Tempo and scene cells form a single undoable transaction.
    pub fn import_retrospective(&mut self, start: f64, end: f64, bars: usize) -> Result<usize, String> {
        let prepared = self.prepare_retrospective(start, end, bars)?;
        self.state.note_audition.stop();
        super::edit::finish_active_gesture(self);
        let checkpoint = self.history.clone();
        let result = (|| {
            self.apply_recorded_scene_structure_mutation("Import MIDI capture", |app| {
                app.restore_scene_structure_state(&prepared.scenes)
            })?;
            super::edit::try_apply_command(self, super::AppCommand::SetBpm { bpm: prepared.bpm })
                .map_err(|error| format!("Could not set loop tempo: {error:?}"))?;
            Ok(prepared.note_count)
        })();
        if let Err(error) = result {
            super::edit::rollback_history_to(self, checkpoint)
                .map_err(|rollback| format!("{error}; undoing the import failed: {rollback:?}"))?;
            return Err(error);
        }
        super::edit::squash_history_since(self, checkpoint.undo_len(), "Import MIDI capture and tempo");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, PatternSnapshot, SequencerState, TrackRegistry};

    fn app() -> App {
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        state.replace_pattern_repository(vec![PatternSnapshot::new_default(2, &[])], 0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(Arc::new(state), LiveGraphPtr(std::ptr::null_mut()), 44_100,
            AudioBuses {
                bus_l_id: 0, bus_r_id: 0, default_bus_nodes: vec![],
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(vec![]))),
                reverb_bus_id: 0, reverb_node_id: 0,
            }, Arc::new(MasterRecorder::new(44_100, 2)), tx);
        app.tracks = vec!["Kick".into(), "Snare".into()];
        app.track_registry = TrackRegistry::for_legacy_track_count(2).unwrap();
        app
    }

    fn draft(app: &mut App) {
        app.retrospective.draft = Some(CaptureDraft {
            duration: 4.0, truncated: false, scene: app.state.current_scene_id().unwrap(),
            notes: vec![
                CapturedNote { track: TrackId(1), transpose: 0.0, velocity: 0.8, start: 1.0, end: 1.1 },
                CapturedNote { track: TrackId(1), transpose: 0.0, velocity: 0.8, start: 1.07, end: 1.18 },
                CapturedNote { track: TrackId(2), transpose: 7.0, velocity: 0.5, start: 2.123, end: 3.5 },
                CapturedNote { track: TrackId(1), transpose: 12.0, velocity: 0.8, start: 3.0, end: 3.2 },
            ],
        });
    }

    /// Hits of a groove repeated `bars` times from `start`, with a small
    /// deterministic timing wobble. `pattern` is (sixteenth, track) per bar
    /// and may span several bars.
    fn groove(start: f64, bpm: f64, pattern: &[(usize, u64)], pattern_bars: usize, repeats: usize) -> Vec<CapturedNote> {
        loose_groove(start, bpm, pattern, pattern_bars, repeats, 0.02)
    }

    /// `slop` is the peak-to-peak timing wobble in seconds.
    fn loose_groove(start: f64, bpm: f64, pattern: &[(usize, u64)], pattern_bars: usize, repeats: usize, slop: f64) -> Vec<CapturedNote> {
        let sixteenth = 15.0 / bpm;
        let mut notes = Vec::new();
        let mut wobble = 0x9e37_79b9u32;
        for repeat in 0..repeats {
            for &(step, track) in pattern {
                wobble = wobble.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let jitter = ((wobble >> 16) as f64 / 65_536.0 - 0.5) * slop;
                let time = start + ((repeat * pattern_bars * 16 + step) as f64) * sixteenth + jitter;
                notes.push(CapturedNote { track: TrackId(track), transpose: 0.0, velocity: 1.0, start: time, end: time + 0.05 });
            }
        }
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        notes
    }

    const BOOM_BAP: &[(usize, u64)] = &[
        (0, 1), (10, 1), (4, 3), (12, 3),
        (0, 2), (2, 2), (4, 2), (6, 2), (8, 2), (10, 2), (12, 2), (14, 2),
    ];

    #[test]
    fn capture_loop_detection_skips_a_stray_hit_and_finds_the_groove_tempo() {
        let mut notes = vec![CapturedNote { track: TrackId(1), transpose: 0.0, velocity: 1.0, start: 1.2, end: 1.25 }];
        notes.extend(groove(9.263, 113.0, BOOM_BAP, 1, 4));
        let guess = detect_capture_loop(&notes).unwrap();
        assert!((guess.start - 9.263).abs() < 0.015, "{guess:?}");
        assert_eq!((guess.bpm, guess.bars), (113, 1));
    }

    /// Slow, hand-played hip-hop: quarter hats, kick on 1 and the and-of-2,
    /// clap on 2 and 4, each hit up to ±35 ms off the grid.
    #[test]
    fn capture_loop_detection_finds_a_loose_slow_groove_without_doubling() {
        let pattern = [(0, 1), (6, 1), (4, 3), (12, 3), (0, 2), (4, 2), (8, 2), (12, 2)];
        for bpm in [72.0, 78.0, 85.0, 92.0] {
            let guess = detect_capture_loop(&loose_groove(3.0, bpm, &pattern, 1, 5, 0.07)).unwrap();
            assert!((guess.bpm as f64 - bpm).abs() <= 1.0, "{bpm}: {guess:?}");
            assert_eq!(guess.bars, 1, "{bpm}: {guess:?}");
        }
    }

    /// Hit times read off a real capture that used to report ~150 BPM.
    #[test]
    fn capture_loop_detection_reads_a_recorded_slow_backbeat() {
        let lanes: [(u64, &[f64]); 3] = [
            (1, &[12.87, 18.99, 20.63, 22.19, 23.82, 25.25]),
            (2, &[13.18, 16.34, 18.07, 18.65, 19.57, 20.46, 21.24, 21.95, 22.80, 23.65, 24.37, 25.05]),
            (3, &[18.21, 19.78, 21.44, 23.04, 24.54]),
        ];
        let notes: Vec<_> = lanes.iter().flat_map(|(track, times)| times.iter().map(|&start|
            CapturedNote { track: TrackId(*track), transpose: 0.0, velocity: 1.0, start, end: start + 0.1 }))
            .collect();
        let guess = detect_capture_loop(&notes).unwrap();
        assert!((70..=90).contains(&guess.bpm), "{guess:?}");
    }

    #[test]
    fn capture_loop_detection_keeps_a_two_bar_variation_whole() {
        let mut pattern = BOOM_BAP.to_vec();
        pattern.extend(BOOM_BAP.iter().map(|&(step, track)| (step + 16, track)));
        pattern.retain(|&(step, track)| !(track == 1 && step == 26));
        pattern.extend([(23, 1), (30, 1), (31, 1)]);
        let guess = detect_capture_loop(&groove(2.0, 128.0, &pattern, 2, 3)).unwrap();
        assert_eq!((guess.bpm, guess.bars), (128, 2));
    }

    #[test]
    fn capture_loop_detection_folds_fast_grooves_into_half_time() {
        // 170 BPM drum-and-bass reads as half time: same loop duration.
        let guess = detect_capture_loop(&groove(0.5, 170.0, BOOM_BAP, 1, 6)).unwrap();
        assert_eq!(guess.bpm, 85);
        let guess = detect_capture_loop(&groove(0.5, 90.0, BOOM_BAP, 1, 4)).unwrap();
        assert_eq!((guess.bpm, guess.bars), (90, 1));
    }

    #[test]
    fn capture_loop_detection_rejects_unsteady_playing() {
        let mut wobble = 0x1234_5678u32;
        let mut time = 0.3;
        let notes: Vec<_> = (0..24).map(|i| {
            wobble = wobble.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            time += 0.15 + (wobble >> 16) as f64 / 65_536.0 * 0.6;
            CapturedNote { track: TrackId(1 + i % 3), transpose: 0.0, velocity: 1.0, start: time, end: time + 0.1 }
        }).collect();
        assert_eq!(detect_capture_loop(&notes), None);
        assert_eq!(detect_capture_loop(&notes[..3]), None);
    }

    #[test]
    fn retrospective_bar_count_keeps_usable_tempos_and_folds_slow_phrases() {
        for (tempo, bars) in [(36.0, 2), (48.0, 2), (20.0, 4),
            (69.49, 2), (69.51, 1), (70.0, 1), (120.0, 1), (180.0, 1)] {
            assert_eq!(capture_bar_count(240.0 / tempo, 1).unwrap(), bars, "{tempo} BPM");
        }
        for requested in 1..=MAX_STEPS / 16 {
            let bars = capture_bar_count(CAPTURE_WINDOW.as_secs_f64(), requested).unwrap();
            assert!(bars <= MAX_STEPS / 16 && 240.0 * bars as f64 / 30.0 >= 70.0);
            assert_eq!(capture_bar_count(30.0, bars).unwrap(), bars);
        }
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY, 1000.0] {
            assert!(capture_bar_count(duration, 1).is_err());
        }
    }

    #[test]
    fn retrospective_slow_tempos_preserve_loop_and_note_times() {
        for (original_bpm, expected_bpm, expected_bars) in [(36, 72, 2), (48, 96, 2), (20, 80, 4)] {
            let mut app = app();
            draft(&mut app);
            let duration = 240.0 / original_bpm as f64;
            app.retrospective.draft.as_mut().unwrap().duration = duration;
            let prepared = app.prepare_retrospective(0.0, duration, 1).unwrap();
            assert_eq!((prepared.bpm, prepared.steps), (expected_bpm, expected_bars * 16));
            let seconds_per_step = 15.0 / prepared.bpm as f64;
            assert!((prepared.steps as f64 * seconds_per_step - duration).abs() < 1e-9);
            for track in 0..2 {
                let data = prepared.scenes.effective_track_pattern(track).unwrap();
                let mut recovered = Vec::new();
                for step in 0..prepared.steps {
                    for voice in 0..data.chord_snapshot.steps[step].len() {
                        let onset = (step as f64 + data.chord_snapshot.delays[step][voice] as f64) * seconds_per_step;
                        let length = data.chord_snapshot.durations[step][voice] as f64 * seconds_per_step;
                        recovered.push((onset, length));
                    }
                }
                let notes: Vec<_> = app.retrospective.draft.as_ref().unwrap().notes.iter()
                    .filter(|note| note.track == app.track_registry.id_at(track).unwrap()).collect();
                assert_eq!(recovered.len(), notes.len());
                for ((onset, length), note) in recovered.into_iter().zip(notes) {
                    assert!((onset - note.start).abs() < 1e-6);
                    assert!((length - (note.end - note.start)).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn retrospective_window_freezes_held_notes_and_expires_old_trigs() {
        let origin = Instant::now();
        let mut capture = RetrospectiveCapture::new(origin);
        capture.note_on(1, TrackId(1), 0.0, 0.7, origin);
        capture.note_off(1, origin + Duration::from_secs(1));
        capture.note_on(2, TrackId(2), 7.0, 0.4, origin + Duration::from_secs(29));
        let snapshot = capture.snapshot(origin + Duration::from_secs(31), SceneId(1)).clone();
        assert_eq!(snapshot.duration, 30.0);
        assert_eq!(snapshot.notes.len(), 1);
        assert_eq!((snapshot.notes[0].start, snapshot.notes[0].end), (28.0, 30.0));
        capture.note_off(2, origin + Duration::from_secs(32));
        assert_eq!(capture.draft.as_ref().unwrap().notes[0].end, 30.0);
        assert!(capture.snapshot(origin + Duration::from_secs(62), SceneId(1)).notes.is_empty());
    }

    #[test]
    fn retrospective_ring_is_bounded_and_reports_capacity_eviction() {
        let now = Instant::now();
        let mut capture = RetrospectiveCapture::new(now);
        for generation in 0..MAX_CAPTURE_NOTES + 7 {
            capture.note_on(generation as u64, TrackId(1), 0.0, 1.0, now);
        }
        assert_eq!(capture.notes.len(), MAX_CAPTURE_NOTES);
        assert_eq!(capture.notes.capacity(), MAX_CAPTURE_NOTES);
        assert!(capture.snapshot(now + Duration::from_secs(1), SceneId(1)).truncated);
        assert!(!capture.snapshot(now + Duration::from_secs(31), SceneId(1)).truncated);
    }

    #[test]
    fn retrospective_release_matches_all_targets_and_fixed_trigs() {
        let now = Instant::now();
        let mut capture = RetrospectiveCapture::new(now);
        capture.note_on(1, TrackId(1), 0.0, 0.7, now);
        capture.note_on(1, TrackId(2), 7.0, 0.7, now);
        capture.note_on(2, TrackId(1), 12.0, 0.8, now);
        capture.note_off(1, now + Duration::from_millis(125));
        capture.trig(TrackId(2), 0.0, now, Duration::from_millis(50));
        let notes = &capture.snapshot(now + Duration::from_millis(500), SceneId(1)).notes;
        assert_eq!(notes.iter().map(|n| n.end).collect::<Vec<_>>(), vec![0.125, 0.125, 0.5, 0.05]);
    }

    #[test]
    fn retrospective_import_preserves_groove_repeats_and_crop_edges_with_undo() {
        let mut app = app();
        app.state.pattern.patterns[0].toggle_step(8);
        let original = app.state.effective_track_pattern_id(0).unwrap();
        draft(&mut app);
        assert_eq!(app.import_retrospective(1.0, 3.0, 1).unwrap(), 3);
        let imported = app.state.effective_track_pattern_id(0).unwrap();
        assert_ne!(original, imported);
        assert!(!app.state.pattern.patterns[0].is_active(8));
        assert_eq!(app.state.pattern.chord_data[0].count(0), 2);
        assert!((app.state.pattern.chord_data[0].get_delay(0, 1) - 0.56).abs() < 1e-6);
        assert!((app.state.pattern.chord_data[1].get_delay(8, 0) - 0.984).abs() < 1e-5);
        assert!((app.state.pattern.chord_data[1].get_duration(8, 0) - 7.016).abs() < 1e-5);
        assert_eq!(app.state.pattern.step_data[1].get(8, StepParam::Velocity), 0.5);
        assert!(app.state.with_pool_pattern(0, original, |data| data.track_bits[0] & (1 << 8) != 0).unwrap());
        super::super::edit::undo(&mut app);
        assert_eq!(app.state.effective_track_pattern_id(0), Some(original));
        assert!(app.state.pattern.patterns[0].is_active(8));
        super::super::edit::redo(&mut app);
        assert_eq!(app.state.effective_track_pattern_id(0), Some(imported));
        assert_eq!(app.state.pattern.chord_data[0].count(0), 2);
    }

    /// A detected crop snaps to the grid line nearest an early first hit, so
    /// it starts after that hit. Import must still put the downbeat on step
    /// one and leave the next bar's early downbeat out of the last step.
    #[test]
    fn retrospective_import_of_a_detected_crop_keeps_an_early_downbeat_on_step_one() {
        const BPM: f64 = 113.0;
        const EARLY: f64 = 0.008;
        let pattern = [(0, 1), (10, 1), (0, 2), (4, 2), (8, 2), (12, 2)];
        let mut notes = loose_groove(9.263, BPM, &pattern, 1, 4, 0.0);
        let bar = 240.0 / BPM;
        for note in &mut notes {
            let in_bar = (note.start - 9.263).rem_euclid(bar);
            if note.track == TrackId(1) && (in_bar < 1e-6 || bar - in_bar < 1e-6) {
                note.start -= EARLY;
                note.end -= EARLY;
            }
        }
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        let first_kick = notes.iter().find(|note| note.track == TrackId(1)).unwrap().start;
        let guess = detect_capture_loop(&notes).unwrap();
        assert!(guess.start > first_kick, "{guess:?} vs first kick {first_kick}");
        let bars = guess.bars;
        let mut app = app();
        let duration = notes.last().unwrap().end + 1.0;
        app.retrospective.draft = Some(CaptureDraft {
            duration, truncated: false, scene: app.state.current_scene_id().unwrap(), notes,
        });
        let end = guess.start + 240.0 * bars as f64 / guess.bpm as f64;
        app.import_retrospective(guess.start, end, bars).unwrap();
        assert!(app.state.pattern.patterns[0].is_active(0));
        assert_eq!(app.state.pattern.chord_data[0].get_delay(0, 0), 0.0);
        assert!(!app.state.pattern.patterns[0].is_active(bars * 16 - 1));
    }

    /// A flam just before the crop start must not move onto a step one that
    /// already has its own hit: that would double the downbeat, or fail the
    /// import on the mismatched velocity.
    #[test]
    fn retrospective_import_leaves_a_pre_start_flam_off_an_occupied_step_one() {
        let sixteenth = 15.0 / 120.0;
        let note = |start: f64, velocity: f32| CapturedNote {
            track: TrackId(1), transpose: 0.0, velocity, start, end: start + 0.05,
        };
        let notes = vec![note(1.0 - 0.2 * sixteenth, 0.4), note(1.0, 1.0), note(1.0 + 8.0 * sixteenth, 1.0)];
        let mut app = app();
        app.retrospective.draft = Some(CaptureDraft {
            duration: 4.0, truncated: false, scene: app.state.current_scene_id().unwrap(), notes,
        });
        app.import_retrospective(1.0, 3.0, 1).unwrap();
        assert!(app.state.pattern.patterns[0].is_active(0));
        assert_eq!(app.state.pattern.chord_data[0].count(0), 1);
        assert_eq!(app.state.pattern.step_data[0].get(0, StepParam::Velocity), 1.0);
    }

    /// Kit pads in a rack with clips read their pattern from the active clip,
    /// so import and preview must write there, not only the scene cell.
    #[test]
    fn retrospective_import_writes_the_active_clip_of_a_clip_bearing_rack() {
        for seeded in [true, false] {
            let mut app = app();
            if seeded { app.state.pattern.patterns[0].toggle_step(8); }
            app.capture_synchronized_scene_structure_state().unwrap();
            app.state.with_scenes_mut(|scenes| scenes.convert_rack_to_clips(9, &[0, 1])).unwrap();
            draft(&mut app);
            let prepared = app.prepare_retrospective(1.0, 3.0, 1).unwrap();
            let previewed = prepared.scenes.effective_track_pattern(0).unwrap();
            assert_eq!(previewed.chord_snapshot.steps[0].len(), 2, "seeded {seeded}");
            assert_eq!(app.import_retrospective(1.0, 3.0, 1).unwrap(), 3);
            assert_eq!(app.state.pattern.chord_data[0].count(0), 2, "seeded {seeded}");
            assert!(!app.state.pattern.patterns[0].is_active(8));
            super::super::edit::undo(&mut app);
            assert_eq!(app.state.pattern.chord_data[0].count(0), 0);
            assert_eq!(app.state.pattern.patterns[0].is_active(8), seeded);
        }
    }

    #[test]
    fn retrospective_import_resolves_stable_tracks_after_reordering() {
        let mut app = app();
        draft(&mut app);
        app.track_registry.move_to(TrackId(1), 1).unwrap();
        app.import_retrospective(1.0, 3.0, 1).unwrap();
        assert_eq!(app.state.pattern.chord_data[1].count(0), 2);
        assert_eq!(app.state.pattern.chord_data[0].get(8, 0), 7.0);
    }

    #[test]
    fn retrospective_audition_leaves_patterns_and_tempo_untouched_import_undo_restores_both() {
        let mut app = app();
        app.state.transport.bpm.store(137, std::sync::atomic::Ordering::Relaxed);
        let original = app.state.effective_track_pattern_id(0);
        draft(&mut app);
        // A four-second phrase starts at 60 BPM, interpreted as two bars at 120.
        app.audition_retrospective(0.0, 4.0, 1).unwrap();
        assert_ne!(app.state.note_audition.generation(), 0);
        assert_eq!(app.state.effective_track_pattern_id(0), original);
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.state.transport.bpm.load(std::sync::atomic::Ordering::Relaxed), 137);
        app.import_retrospective(0.0, 4.0, 1).unwrap();
        assert_eq!(app.state.note_audition.generation(), 0);
        assert_eq!(app.state.transport.bpm.load(std::sync::atomic::Ordering::Relaxed), 120);
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 32);
        assert_eq!(app.history.undo_len(), 1);
        super::super::edit::undo(&mut app);
        assert_eq!(app.state.effective_track_pattern_id(0), original);
        assert_eq!(app.state.transport.bpm.load(std::sync::atomic::Ordering::Relaxed), 137);
        super::super::edit::redo(&mut app);
        assert_eq!(app.state.transport.bpm.load(std::sync::atomic::Ordering::Relaxed), 120);
        assert_ne!(app.state.effective_track_pattern_id(0), original);
    }

    #[test]
    fn retrospective_invalid_imports_leave_patterns_and_draft_intact() {
        let mut app = app();
        draft(&mut app);
        let original = app.state.effective_track_pattern_id(0);
        for (start, end, bars) in [(f64::NAN, 3.0, 1), (3.0, 1.0, 1), (4.5, 6.0, 1),
            (0.0, 3.0, 0), (0.0, 3.0, 17), (0.0, 0.5, 1)] {
            assert!(app.import_retrospective(start, end, bars).is_err());
        }
        app.retrospective.draft.as_mut().unwrap().notes[1].velocity = 0.2;
        assert!(app.import_retrospective(1.0, 3.0, 1).unwrap_err().contains("velocities"));
        app.retrospective.draft.as_mut().unwrap().scene = SceneId(999);
        assert!(app.import_retrospective(1.0, 3.0, 1).unwrap_err().contains("scene changed"));
        assert_eq!(app.state.effective_track_pattern_id(0), original);
        assert!(app.retrospective.draft.is_some());
    }
}
