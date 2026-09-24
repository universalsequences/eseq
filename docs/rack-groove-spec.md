# Rack Grooves — Extracted Feel, Applied to Every Trig Source

Status: rev 1, unbuilt. Epic: `eseq-groove` (slices `.1`–`.7` below).

## Problem

Graph sequencers (`alez.neural/variable-reset` and friends) produce rhythms
that are structurally rich but rhythmically straight. Two reasons, one in the
code and one in the model:

1. **Graph-mode emissions are never swung.** The graph block in
   `scheduler/lookahead.rs` (the `// Graph-mode sequencers:` loop) enqueues
   `emission.sample_time` exactly as the runtime produced it — on the node's
   `:quantize` boundary. The legacy neural path does call
   `swung_network_sample_time` (`scheduler/clock.rs`), but graph sequencers
   don't go through it.
2. **Swing is the wrong shape anyway.** A Dilla/Madlib pocket is not a swing
   percentage. It is a per-instrument, per-position timing map: hats late on
   the beat, kick slightly early, snare dragging, some 16ths pushed and others
   not. It can't be factorized into `swing` + `swing_resolution`. Per-neuron
   swing params would get to UK-garage level and stop there.

The user already produces this feel by playing a drum rack by hand, recording
unquantized with Capture MIDI (`app/retrospective.rs`), and sending it to
patterns. The feel then lives in the pattern data and nowhere else. It can't be
reused by a different pattern, let alone by a generative sequencer.

## Idea

A **groove** is a timing (and accent) map extracted from a played pattern, and
it belongs to the **rack**, not to any sequencer. It is applied at the point
where a trig aimed at a rack pad becomes a sample time. Every trig source that
targets the rack's pads plays through it without knowing it exists:

- the member tracks' step sequencers,
- rack-owned graph sequencers (`ProjectRackConfig::sequencers`),
- project-owned graph sequencers routed at rack members.

The workflow this enables:

1. Play a rack by hand, Capture MIDI, send to patterns.
2. **Extract Groove** from those patterns into the rack's groove list.
3. Pick that groove on the rack. Punch in a new step pattern: it swings.
4. Delete the pattern and attach a neural sequencer: it swings the same way.

Swing becomes a special case: a two-slot, one-row groove. MPC-style swing
ships as built-in generic grooves.

## Non-goals

- Audio-based groove extraction (onset detection on audio). Extraction reads
  pattern data only.
- Grooves on non-rack tracks. A plain track can be wrapped in a rack if it
  wants a groove. Revisit only if that proves awkward.
- Changing what a graph sequencer *decides*. Energy, thresholds, max-poly
  selection and resets all still run on the straight grid. The groove moves
  only *when* the decided trigs sound, plus an optional velocity accent.
- Per-clip groove selection (rack clips). The groove is rack-level in rev 1.

## Data model

```rust
/// One extracted or built-in feel. Positions are in BEATS within the period,
/// never in steps: member tracks can have different timebases, and graph
/// sequencers fire at their own `:resolution`/`:quantize`.
pub struct ProjectGroove {
    pub id: GrooveId,                 // stable within the rack
    pub name: String,
    pub period_beats: f64,            // 4.0 = one bar of 4/4, 8.0 = two bars
    pub resolution_beats: f64,        // slot spacing; 0.25 = 16ths
    /// Per-pad rows keyed by `pad_note` (stable across member reorder),
    /// not by member index.
    pub pad_rows: Vec<GroovePadRow>,
    /// All-pads row: median over every pad at each slot. The fallback for pads
    /// without a row and the only row a generic groove has.
    pub shared_row: GrooveRow,
}

pub struct GroovePadRow {
    pub pad_note: i32,
    pub row: GrooveRow,
}

/// `slots.len() == round(period_beats / resolution_beats)`.
pub struct GrooveRow {
    pub slots: Vec<GrooveSlot>,
}

pub struct GrooveSlot {
    /// Offset from the slot's straight position, in units of
    /// `resolution_beats`. Signed: negative = early. Extraction keeps it in
    /// [-0.5, 0.5) (see Extraction §snap).
    pub offset: f32,
    /// Accent relative to the row's median velocity (1.0 = neutral).
    pub velocity_scale: f32,
    /// Robust spread of `offset` across repeats (MAD), for the Random amount.
    pub spread: f32,
    /// How the slot got its value; empty slots are filled, not zero.
    pub source: GrooveSlotSource,     // Measured | FilledFromNeighbors | FilledFromShared | Zero
}
```

Stored on the rack:

```rust
pub struct ProjectRackConfig {
    // ... existing fields ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grooves: Vec<ProjectGroove>,
    #[serde(default)]
    pub groove: RackGrooveSettings,
}

pub struct RackGrooveSettings {
    pub active: Option<GrooveRef>,    // Rack(GrooveId) | Builtin(&'static str id)
    pub timing_amount: f32,           // 0..1.5, default 1.0
    pub velocity_amount: f32,         // 0..1.5, default 0.0 (timing-only by default)
    pub random_amount: f32,           // 0..1.0, default 0.0
}
```

- Built-in generic grooves (MPC swing 50–75% at 16ths and 8ths) live in code or
  `content/`, not in the project. They have a `shared_row` and no pad rows.
- Grooves travel with kit presets (`KIT_PRESET_VERSION` bump) so a saved kit
  brings its pocket. This is what "the grooves available for this rack" means.
- Follow the project's serialization/versioning convention
  (`PROJECT_FILE_VERSION`); every new field is `serde(default)`, so older
  projects load without grooves.

### Scheduler snapshot

The scheduler needs `member track → (resolved groove row, settings)` with no
lookups by pad note on the hot path. Add a pre-resolved table to
`SequencerSnapshot`, built when rack config or rack membership changes. The
snapshot already carries `rack_memberships`.

```rust
pub struct TrackGrooveSnapshot {
    pub period_beats: f64,
    pub resolution_beats: f64,
    pub row: Arc<GrooveRow>,          // pad row, or shared_row if the pad has none
    pub timing_amount: f32,
    pub velocity_amount: f32,
    pub random_amount: f32,
}
// SequencerSnapshot: pub track_grooves: Vec<Option<TrackGrooveSnapshot>>  (parallel to tracks)
```

`None` for tracks outside a rack, or in a rack with no active groove, keeps
today's behavior bit-for-bit.

## Extraction

**Action:** "Extract Groove…" on a rack, next to the pattern/rack menus. It opens
a small modal with a name, period (1 or 2 bars), resolution (16th default, 32nd
option), and "Quantize source afterwards" (default on).

**Source:** the rack members' current effective patterns, or a rack clip. For
each member mapped to a pad, read every active step and compute the **heard**
beat position:

```
beat = step_start_beats(step, member timebase)
     + delay * step_beats                       // StepParam::Delay, or each
                                                // chord_snapshot.delays[n]
     + track swing delay for that step          // the pattern's own swing is part
                                                // of what the user heard
```

Capture MIDI stores sub-step timing as per-note `chord_snapshot.delays`
(`app/retrospective.rs`: `step = floor(position)`, `delay = fraction`). Plain
step edits use `StepParam::Delay`. Read both.

**Snap:** assign each hit to the **nearest** slot at `resolution_beats`, not the
floor slot. `offset = (beat - slot_beat) / resolution_beats`, in [-0.5, 0.5).
This turns a hit the recorder stored as "very late on the previous step" into
"slightly early on the right step", which is the correct reading of a kick
pushed ahead of the beat.

Known ambiguity: a hat dragged by more than half a slot snaps to the next slot
as an early hit. Rev 1 accepts this. If it turns out to matter, add a "late
bias" snap window (e.g. [-0.35, 0.65)) to the modal.

**Aggregate:** fold positions modulo `period_beats`. For each `(pad, slot)`:
- `offset` = median across repeats,
- `spread` = median absolute deviation,
- `velocity_scale` = median velocity at the slot ÷ that pad's median velocity.

If two hits from one pad land in the same slot in one repeat (flam, ghost
pickup), keep the louder one for timing and velocity.

**Shared row:** the same aggregation over all pads pooled.

**Fill empty cells.** This matters: a generative sequencer will fire where you
never played. For each pad row, an unmeasured slot takes, in order:
1. the same slot's value in the same pad row at the **same position in the other
   half of the period** (for a 2-bar period),
2. linear interpolation between the pad's nearest measured slots **of the same
   metric class** (on-beat, &, e/a positions),
3. the shared row's value at that slot,
4. zero.

`source` records which rule filled the slot, so the UI can dim guessed cells.

**Quantize source (optional, default on, one undo step):** zero the Delay
p-locks and chord delays on the source members and set their swing to 50, then
activate the new groove. The pattern should then sound the same through the
groove as it did before. That equivalence is the main acceptance test for
extraction (within slot-median rounding).

Without this option, re-applying the groove to its own source doubles the
feel, since the stored delays and the groove offset add.

## Application

One function, called at every site where a trig aimed at a rack member gets its
sample time:

```
fn grooved_sample_time(g: &TrackGrooveSnapshot, boundary_beats, straight_sample,
                       samples_per_quarter, seed) -> (u64, f32 /*vel mult*/)
  pos   = boundary_beats.rem_euclid(g.period_beats) / g.resolution_beats
  k     = floor(pos); t = pos - k
  off   = lerp(row[k].offset, row[k+1 mod n].offset, t)          // time-warp between slots
  off  += g.random_amount * row[k].spread * hash_noise(seed)      // deterministic
  off  *= g.timing_amount
  vel   = lerp(1.0, lerp(row[k].velocity_scale, row[k+1].velocity_scale, t), g.velocity_amount)
  time  = straight_sample + off * resolution_beats * samples_per_quarter
```

- **Interpolation** is what lets a 16th-resolution groove shape 32nd-note hats
  or triplet-quantized neurons. Swing is exactly this warp with two slots.
- **Beat frame:** `boundary_beats` is transport beats, bar-aligned. That is the
  same frame graph sequencers quantize in. It is *not* the member track's local
  cycle beat: a groove is a pocket relative to the bar, so a 7-step polymetric
  hat member should still sit late on the downbeat. Arrangement/clip anchoring
  must use the same beat the step scheduler already resolves for swing buckets.
  Implementation must verify this with an anchored-song test.
- **Random** is seeded by `(absolute boundary index, pad_note)`. That way
  offline render and bounce are reproducible while each bar still varies.
- **Velocity** multiplies the source's resolved velocity and is clamped to its
  valid range.
- **Duration is untouched.** A trig carries its duration, so moving the trig
  moves the whole note. Retrig offsets are measured from the trig and move with
  it.

### Sites

1. **Step trigs:** `scheduler/lookahead.rs` step loop, where `sample_time` is
   built from `delayed_step_sample_time` plus the swing block. When the member
   has a groove, the groove **replaces** the track-swing and step-swing-override
   delay. Step Delay / chord delays still **add** on top: they are intentional
   per-hit nudges inside the pocket.
2. **Graph-mode emissions:** the graph block in `scheduler/lookahead.rs`, before
   `enqueue_emitted_network_event_with_midi_fx`, keyed on `emission.event.track`.
3. **Legacy neural outputs:** replace `swung_network_sample_time` with the
   groove when the target track has one. Otherwise keep today's swing behavior.
4. **Process emissions** (`enqueue_due_process_emissions`) that target a rack
   member: apply the groove, so step processes behave like the sources above.
5. **Roll hits** (`roll_swung_sample_time`) and **live-keyboard record**: the
   groove replaces swing the same way. Recording through a grooved rack must
   **unwind** the groove offset before storing phase, or playback applies it
   twice. This is the same bug class as eseq-k0v8 for swing, so fix them
   together.

MIDI fx order: the groove applies to the trig's source time **before** the
member's MIDI fx chain, the same place swing applies today. A member MIDI-fx
quantizer will re-straighten grooved trigs. That is expected and is the
quantizer's job. Document it in the UI hint.

### Early hits

Negative offsets schedule a trig **before** its straight boundary. The
scheduler finds trigs per chunk `[chunk_start, chunk_end)`, so a trig whose
boundary lies just past `chunk_end` may need to sound inside this chunk.

Requirement: for every source, trigs are discovered at least
`E = max early offset (beats)` ahead of the chunk edge they must be enqueued in:
- step triggers: extend the trigger search window by `E` and skip already-handled
  boundaries in the next chunk;
- graph runtimes: run the runtime's cursor `E` ahead. Graph state stays
  consistent because boundaries are still processed in order, just sooner.

`E` is bounded: snapping keeps `|offset| < 0.5` slot, and `timing_amount <= 1.5`
gives `E <= 0.75 * resolution_beats`.

Until that slice lands, clamp applied offsets to `>= 0` (grooves play correctly
for late feels, and early hits land on the grid).

Must hold: an early trig is never enqueued at a sample the audio thread has
already passed. Add a test that pins this at the smallest supported buffer
size.

## UI

On the drum rack panel (`content/ui/drum-rack-v2.lisp`), a **Groove** section:
- picker listing *This rack* grooves, then *Generic* built-ins, then Off,
- Timing / Velocity / Random amount knobs (p-lockable/automatable like other
  rack params; rev 1 may ship them as plain values),
- a compact heatmap: rows = pads, columns = slots, color = offset
  (early ↔ late), with filled (guessed) cells dimmed,
- "Extract Groove…" and rename/delete for rack grooves.

Member tracks with an active rack groove show their swing control disabled with
a "groove" hint, so there is one visible source of truth for the feel.

## Slices

1. **Groove model + extraction.** Data types, serde, rack storage, pure
   extraction (heard-position math, nearest snap, median/MAD, fill rules,
   shared row), and the quantize-source edit as one undo step. Heavily unit
   tested on synthetic patterns. No playback change.
2. **Apply, late-only, all sources.** `track_grooves` snapshot table, the
   `grooved_sample_time` function, wired at step, graph-emission, legacy-neural
   and process sites with offsets clamped `>= 0`. Built-in MPC swing grooves.
   After this slice, the headline workflow works for late feels.
3. **Early offsets.** Lookahead discovery `E` ahead for step triggers and graph
   runtimes; remove the clamp; no-past-sample test.
4. **Rack panel UI.** Groove section, Extract Groove modal, heatmap, member swing
   hint.
5. **Velocity + random amounts.** Velocity scaling and deterministic jitter.
6. **Record/roll unwind.** Roll and live-record through a grooved rack store
   straight phase. Pair with eseq-k0v8.
7. **Kit preset carry + cross-rack.** Grooves in kit presets; applying another
   rack's groove maps pad rows by `pad_note`, falling back to the shared row.

## Acceptance

- Extract from a captured Dilla-style take, quantize source, play: the take
  sounds as it did before extraction (per-hit error ≤ the slot median residual).
- Same rack, source pattern deleted, variable-reset attached: the hats sit late
  on the downbeats the way the take's hats did, and kicks land where the take's
  kicks landed.
- Rack with no active groove: scheduler output is bit-identical to today
  (existing swing/neural tests untouched).
- Offline render of a grooved rack with Random > 0 is reproducible run to run.
