# Rack Grooves — Extracted Feel, Applied to Every Trig Source

Status: rev 2. Slices 1–7 built (model/extraction, application incl. early offsets, rack panel UI, velocity/random, record/roll unwind, kit carry and cross-rack). Rev 2 (§Groove pool, library and pad roles; slices 8–11 = beads `.9`–`.12`) moves grooves off the rack into a project pool backed by a factory/user groove library, adds typed pad roles for cross-kit row matching, and adds a Grooves sidebar tab. Where rev 2 contradicts the rev-1 text below (rack-owned `grooves`, `GrooveRef::Rack`, built-ins in code, the rack-panel heatmap), rev 2 wins. Epic: `eseq-groove`.

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
   *Built (eseq-groove.2):* sites 1–4 plus generator emissions share
   `groove::groove_offset_samples` (`groove/apply.rs`), keyed on the trig's
   straight transport beat: `SnapshotTrigger::boundary_beats` for steps,
   `GraphEmission::grid_beats` (post-`:quantize`, pre-node-swing) for graph
   fires, `item.beat` for process events. A graph node's own `:swing` still
   adds on top of the groove, like step Delay. The per-track table is
   rebuilt by `App::publish_rack_choke_runtime`, the group-topology funnel.
5. **Roll hits** (`roll_swung_sample_time`) and **live-keyboard record**: the
   groove replaces swing the same way. Recording through a grooved rack must
   **unwind** the groove offset before storing phase, or playback applies it
   twice. This is the same bug class as eseq-k0v8 for swing, so fix them
   together.
   *Built (eseq-groove.6 + eseq-k0v8):* one inverse,
   `groove::unwind_step_feel` (`groove/unwind.rs`), maps a HEARD position to
   the straight `(step, phase)` playback moves back onto it. Playback shifts
   a stored hit by the feel of the step it sits on (the groove pocket at the
   step's straight transport boundary, else the track swing of the step's
   bucket with per-step swing p-locks), so the inverse tries every step and
   keeps `heard - shift[s]` when it falls inside step `s`. Overlapping
   readings (an early step reaching back into the one before) keep the
   smaller phase; the gap a later-than-previous step opens reads as early
   for that step (phase 0); a straight Sync wait stays unresolved as before.
   The unwind uses the deterministic pocket (`pocket_offset_beats`), not
   Random's jitter, so one bar's noise is never printed. Live record:
   `SequencerState::record_position_at_beat` (audio stamps, press estimate
   and frontier fallback all go through it). Roll: a grooved member's hit
   plays through `grooved_sample_time` keyed on its grid line (replacing
   swing, floored like other sites, plus the velocity accent), and
   `roll_record_position` unwinds `roll_heard_beats`, so a 32nd roll on 16th
   steps records the delay that replays each hit where it sounded. Pinned by
   `scheduler::tests::rack_groove::{live_record_*, roll_through_a_feel_*,
   grooved_roll_*}` (record, write, replay through the scheduler, within one
   sample) and `groove::tests::unwind_*`.

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

Must hold: an early trig is never enqueued at a sample the audio thread has
already passed. Add a test that pins this at the smallest supported buffer
size.

*Built (eseq-groove.3):* the late-only clamp is gone; `offset_beats` is signed
and floored at `-MAX_EARLY_SLOTS` (0.75) slots, which is what bounds `E` even
under Random jitter. `TrackGrooveSnapshot::max_early_beats` is the exact lead
of one groove (lowest `min(off[k], off[k+1]) - random * spread[k]` after
timing) and `SequencerSnapshot::groove_early_lead_beats` the table's maximum.
`schedule_playing_lookahead` extends its horizon by `ceil(E * spq)` samples,
which is the discovery requirement for every source at once: the step clock's
trigger window, graph runtimes (their boundaries run `E` sooner, in order, so
runtime state matches a non-ahead run to the same beat), and the
process/neural/generator layers. The scheduling frontier is the dedupe: the
next call starts where this one stopped, so no boundary is handled twice, and
no per-source "already handled" bookkeeping can drift across song rows,
launches or roll windows. Every site passes a `GrooveFloor`
(`SnapshotSequencerClock::groove_floor(rendered)`): an early offset never
lands before the audio frontier `rendered` (the transport-start downbeat and
the first chunk after a seek, or a groove made early mid-play, are the cases
it clamps). The one place the frontier stops being a dedupe is a mid-play
resync (topology change, pattern-epoch bump, pattern switch, live-MIDI-FX
toggle): the queue is cleared and the clock rewound to `rendered`, so it finds
again the boundaries of early hits that already SOUNDED before `rendered`.
`seek_to_rendered_position` records the old frontier as
`GrooveFloor::replayed_until`, and an early trig whose straight sample is
below it and whose move lands before `rendered` is dropped (`None`), not
clamped, so it is not played twice. Transport start and other seeks reset the
window to zero. `E` is zero for late-only and ungrooved projects, so they
schedule bit-identically. The cost is up to `E` extra lookahead (at most 0.75
slot) while an early groove plays. The offline renderer accepts a frontier
past its horizon. Pinned by `scheduler::tests::rack_groove::early_groove_*`
(16-frame offline advances, and a resync inside an early window),
`graph_runtime_ahead_by_the_lead_matches_a_non_ahead_run` (against a grooved
and an ungrooved single-call reference) and
`groove::tests::grooved_sample_time_*`.

*Known gaps (eseq-groove.8):* the same resyncs still lose (1) a LATE hit whose
straight boundary is before `rendered` but whose grooved sample is after it
(eseq-groove.2), and (2) graph emissions already produced for the discarded
lookahead. The pattern-epoch and live-MIDI-FX resyncs do not rewind graph
runtimes. That loss predates grooves, but the early lead `E` widens its
window by up to 0.75 slot.

## Groove pool, library and pad roles (rev 2)

Rev 1 stored grooves on the rack. That makes the question "which grooves do I
have, and where is each one applied?" unanswerable without opening every rack,
and it makes a groove extracted on one kit a second-class citizen on another.
Rev 2 follows Ableton's groove pool and eseq's own content tiers. Rev-1 rack
storage never shipped (the branch was unmerged), so there is no migration of
rack-owned grooves; `PROJECT_FILE_VERSION` is bumped once for the pool.

### Three tiers

| Tier | Where | Mutable | Used by playback |
|---|---|---|---|
| Project pool | `Project::grooves: Vec<ProjectGroove>` | yes | yes — the only tier racks reference |
| User library | `AppPaths::user_grooves_dir()` = `user_data_root()/grooves/*.groove` | yes | no |
| Factory library | `AppPaths::grooves_dir()` = `factory_root()/grooves/*.groove` (bundle `content/grooves/`) | no | no |

- This mirrors kits (`kits_dir` / `user_kits_dir`), presets, effects and
  instruments. The Grooves tab lists user + factory merged, like
  `project::list_kit_presets`.
- A `.groove` file is a versioned JSON `ProjectGroove` without `id`
  (`GROOVE_FILE_VERSION`), named by its file stem unless `name` is set.
- The rev-1 built-in MPC swings become factory `.groove` files
  (`mpc-swing-54-16th.groove`, …). `GrooveRef::Builtin` is deleted.
- **Copy-on-apply.** Applying a library groove to a rack first imports it into
  the project pool (reusing an existing pool groove with the same feel —
  `ProjectGroove::same_feel`), then points the rack at the pool id. A project
  therefore plays identically on a machine without that library file, and
  editing the library never changes an existing project. "Save to Library"
  is the reverse copy; it never links.
- `GrooveId` is unique within the project pool.
- `RackGrooveSettings::active: Option<GrooveId>` (a pool id). Deleting a pool
  groove that racks use asks first and turns it off on those racks, as one
  undo step. Extract Groove adds the result to the pool and, with Quantize
  source on (the modal's default), activates it on the source rack in the
  same undo step; without quantizing it is only added, since playing it over
  its own unquantized source would double the feel.
- **Kit presets** carry a copy of the kit's active groove (plus its settings),
  not a list. Loading a kit imports that copy into the pool through the same
  dedupe as copy-on-apply. `KIT_PRESET_VERSION` bumps; v5 kits (rack groove
  list + selection) load by importing their selected groove only.
- *Built (eseq-groove.9):* the pool is `ProjectFile::grooves` / `App::grooves`
  (`PROJECT_FILE_VERSION` 15, the one bump over main; a rev-1 `{"rack": id}`
  reference reads as no groove rather than failing the load) and rides undo
  in `BusGroupStructureState::grooves`. Pool helpers live in
  `groove/pool.rs` (`import_groove` = copy-on-apply dedupe,
  `repair_groove_pool`, `ProjectRackConfig::repair_groove_selection`), the
  library in `groove/library.rs` (`GrooveFile`, `GROOVE_FILE_VERSION` 1;
  list/load/save/rename/delete with `*_in(dir)` cores; entries sorted by
  stem; save never overwrites, it picks `name-2`, ...), kit carry in
  `groove/kit.rs`. Kit version 6 writes `groove: {groove, timing_amount,
  velocity_amount, random_amount}`; a v5 kit's `groove` (a selection) and
  `grooves` (its list, read only) resolve through
  `ProjectKitPreset::carried_groove`, a rev-1 built-in id to the same MPC
  swing. Factory swings are `content/grooves/mpc-swing-<pct>-<16th|8th>.groove`,
  generated by `groove::mpc_swing_groove` (a test keeps them in sync).
  Picker keys are `pool:<id>`, `factory:<stem>`, `user:<stem>`, `off`
  (`GrooveChoice`); `set-rack-groove` with a library key copies on apply.
  `rename-rack-groove` / `delete-rack-groove` act on the pool (group id
  optional); `save-groove-to-library`, `rename-library-groove`,
  `delete-library-groove` edit user files. The host also publishes
  `SEQ.groove-pool` (grooves with their rack instances) and
  `SEQ.groove-library`; the rack entry's `:active-groove-id` is the pool id.
- The scheduler is unchanged: `track_groove_snapshots` resolves each member's
  row from the pool groove its rack references.

### Pad roles (typed slots)

`pad_note` identifies a pad within one kit; across kits it only means the same
drum if both kits follow the same layout. Rev 2 adds an explicit, optional
drum role per pad:

```rust
pub enum PadRole {
    Kick, Snare, Rim, Clap, ClosedHat, PedalHat, OpenHat,
    TomLow, TomMid, TomHigh, Crash, Ride, Shaker, Perc,
}
// ProjectRackPad and ProjectKitPad:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub role: Option<PadRole>,
```

**Standard layout.** A pad without an explicit role gets one inferred from its
`pad_note` using the General MIDI drum map shifted so C4 = pad note 0
(`gm_note - 36`): 0 kick (C4), 1 rim, 2 snare (D4), 3 clap, 4 snare,
5 tom-low, 6 closed-hat (F#4), 7 tom-low, 8 pedal-hat, 9 tom-mid,
10 open-hat (A#4), 11 tom-mid, 12 tom-high, 13 crash, 14 tom-high, 15 ride,
16 crash, 17 ride, 18 shaker, 19 crash, 20 perc; anything else has no role.
Kits authored in this layout (factory kits, eseq-2k9p.25, should) need no
tagging; other kits set roles explicitly. `effective_role(pad)` = explicit role
else inferred.

**Groove rows record roles.** Extraction stores the source pad's effective role
on each `GroovePadRow` (`role: Option<PadRole>`, serde default). Row lookup for
a member pad (`ProjectGroove::row_for_pad(pad_note, role)`), in order:

1. a row with the same `pad_note` whose role is compatible with the pad's
   effective role — equal, or unknown on either side (a row recorded without
   a role, or a pad with none) — the same kit, or a kit in the same layout;
2. a row with the same role (first by `pad_note` order) — the snare row lands
   on this kit's snare wherever it sits;
3. the shared all-pads row.

*Built (eseq-groove.10):* `PadRole` lives in `crate::pad_role` (re-exported
from `project`; serde keys kebab-case, `closed-hat`), with
`PadRole::standard(pad_note)` the layout table and `effective_role()` on
`ProjectRackPad` / `ProjectKitPad`. The lookup is
`ProjectGroove::resolve_pad_row(pad_note, role)` (`row_for_pad` falls back to
the shared row); `track_groove_snapshots`, `groove_row_mapping` and the rack
panel heatmap all use it. No file-version bump: every new field is
`serde(default)` and skipped when `None`. Kit save/load (new rack and
audition) carries explicit roles. `App::set_rack_pad_role_recorded` is one
undo step through the bus/group funnel, which republishes the groove table;
host command `set-rack-pad-role {group-id pad-note role}` takes a role key or
`standard`, and shares `apply_rack_pad_map_command` with the capture harness.
`SEQ.groups` pads carry `:role` (explicit key, "" = Standard), `:role-tag`,
`:role-label` and `:standard-role-label`. The pad cell's right-click opens
the pad menu (Role ▸ Standard (<inferred>), then every role), mounted in the
*fx* rack panel; the tag (BD, SD, CH, ...) sits top-right, bright when
explicit, dim when inferred. Capture fixture:
`crates/sequencer/ui/capture-fixtures/rack-pad-roles.lisp`.

Roles are general pad metadata; grooves are their first consumer. Pattern
transfer between kits, MIDI note maps and Jev can use them later.
Auto-suggesting a role from sample names or the sound classifier is a
follow-up, not rev 2.

### Out of scope for rev 2

- **Commit** (bake a groove into member step `Delay` p-locks and turn it off).
  Only step patterns could be baked — graph, neural and process emissions have
  no stored notes — so on a mixed rack it would half-work. Revisit as a
  step-only action if wanted.
- Editing groove cells by hand.

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

**Rev 2 UI.**

*Grooves sidebar tab* (a browser tab beside Packages, same tree widget and
conventions — see the Packages tab: header rows, `:status-icon`,
`:on-right-click` context menus routed to host commands):
- Header sections **In use** / **Project** / **Library** (user) / **Factory**.
- A project groove row expands to its instances, one row per rack using it:
  `<rack name> · T 100% V 40% R 0%`. Clicking an instance focuses that rack.
  In use lists only pool grooves with at least one instance.
- Selecting a groove shows its pads × slots heatmap (rows labelled by role
  where known, filled cells dimmed) and its period/grid below the tree.
- Context menus. Project groove: Apply to Selected Rack, Rename, Duplicate,
  Save to Library, Delete (confirms and lists affected racks when in use).
  Library/factory groove: Apply to Selected Rack (copy-on-apply), and for
  user files Rename and Delete. All edits are single undo steps; library file
  edits are not undoable and say so in their confirm.

*Rack panel* keeps only: the groove picker (pool grooves, then a *Library*
section whose entries copy-on-apply, then Off), the Timing / Velocity /
Random knobs, "Extract Groove…", and a "Grooves tab" link that opens the tab
with this rack's groove selected. The heatmap and rename/delete move to the tab.

*Pad role* is set from the rack pad's context menu (Role ▸ …, with
"Standard (<inferred>)" as the default entry), and shown as a short tag on
the pad.

## Slices

1. **Groove model + extraction.** Data types, serde, rack storage, pure
   extraction (heard-position math, nearest snap, median/MAD, fill rules,
   shared row), and the quantize-source edit as one undo step. Heavily unit
   tested on synthetic patterns. No playback change.
2. **Apply, late-only, all sources.** `track_grooves` snapshot table, the
   `grooved_sample_time` function, wired at step, graph-emission, legacy-neural
   and process sites with offsets clamped `>= 0` (lifted by slice 3). Built-in
   MPC swing grooves.
   After this slice, the headline workflow works for late feels.
3. **Early offsets.** Lookahead discovery `E` ahead for step triggers and graph
   runtimes; remove the clamp; no-past-sample test. *Built (eseq-groove.3):*
   see §Early hits.
4. **Rack panel UI.** Groove section, Extract Groove modal, heatmap, member swing
   hint.
   *Built (eseq-groove.4):* `content/ui/effects/rack-groove.lisp` renders the
   section beside the pad grid in the rack's *fx* panel; lookups and host
   commands live in `eseq.drum-rack-v2` (`extract-groove`, `set-groove`,
   `set-groove-amount`, `rename-groove`, `delete-groove`). The host publishes
   `SEQ.rack-grooves` (structural: picker labels + keys `rack:<id>` /
   `builtin:<id>` / `off`, active groove, heatmap rows All + pads with
   per-cell offset and measured flags; sub-bar grooves tile to one bar) and
   scalar `SEQ.rack-groove-{timing,velocity,random}-<gid>` the knobs bind to,
   so a drag never rebuilds its section. Commands go through
   `ui/host_commands/rack_grooves.rs`; an amount drag writes through live and
   lands as ONE undo step when the gesture ends
   (`edit::apply_rack_groove_amount_drag`, the `ProcessLaneDrag` shape).
   Rename/delete are one step each; deleting the active groove turns it off.
   A grooved member's track panel shows swing as a "groove" hint. Capture
   fixture: `crates/sequencer/ui/capture-fixtures/rack-groove-panel.lisp`
   (new `(drum-rack TRACK...)` capture form).
5. **Velocity + random amounts.** Velocity scaling and deterministic jitter.
   *Built (eseq-groove.5):* `TrackGrooveSnapshot::offset_beats` adds
   `random * spread[k] * groove_hash_noise(absolute slot, pad_note)` before the
   timing amount (a splitmix hash, no RNG state, so any render from any start
   point is reproducible); `pad_note` rides on the snapshot, and a padless
   member seeds with `padless_seed_key(member)`, below the pad-note domain.
   `apply_velocity` scales by `lerp(1, lerp(scale[k], scale[k+1], t), amount)`
   clamped to Velocity's 0..1 and is a bit-for-bit no-op at amount 0. Every
   site calls `scheduler::grooved_velocity` at the same straight beat that keys
   its timing, exactly once per sounding event: the base step trig AFTER its
   process chain (and before the accumulator), graph and generator emissions,
   legacy neural outputs, and process steps/emissions at enqueue. The process
   chain reads the straight velocity, as it reads straight timing: a ratchet
   or `emit` built from the step's velocity is a process event that enqueue
   grooves at its own beat, so grooving the step first would scale it twice.
6. **Record/roll unwind.** Roll and live-record through a grooved rack store
   straight phase. Pair with eseq-k0v8.
   *Built (eseq-groove.6, with eseq-k0v8):* see §Sites 5.
7. **Kit preset carry + cross-rack.** Grooves in kit presets; applying another
   rack's groove maps pad rows by `pad_note`, falling back to the shared row.
   *Built (eseq-groove.7):* `KIT_PRESET_VERSION` 5 adds
   `ProjectKitPreset::grooves`/`groove`. Loading a kit as a new rack installs
   them (ids re-derived, selection re-pointed); auditioning a v5 kit onto a
   rack ADDS its grooves beside the rack's own (an identical groove is reused,
   not duplicated) and takes the kit's selection and amounts; a pre-v5 kit
   leaves the rack's grooves alone. `App::apply_rack_groove_from_rack_recorded`
   copies another rack's groove into the target and activates it (one undo
   step). Both go through `groove::import_grooves`
   (`groove/transfer.rs`); no per-pad remap is stored, because the scheduler
   table already resolves each member's row by its own pad note.

8. (eseq-groove.9) **Project groove pool + library.** `Project::grooves`, `active: Option<GrooveId>`,
   delete `GrooveRef::Builtin`/rack storage, `grooves_dir`/`user_grooves_dir`,
   `.groove` file format + list/save/delete, factory MPC swing files,
   copy-on-apply with dedupe, kit presets carry the active groove copy.
   Existing host commands and the rack panel keep working against the pool.
   *Built (eseq-groove.9):* see §Three tiers. Until slice 11 the rack
   panel's picker already lists pool grooves, then the library, then Off.
9. (eseq-groove.10) **Pad roles.** `PadRole`, `role` on rack + kit pads, standard-layout
   inference, role recorded on extracted rows, role-aware row lookup in the
   scheduler table, pad context-menu role picker + pad tag.
10. (eseq-groove.11) **Grooves sidebar tab.** Tree, instances, heatmap preview, context menus,
    host commands shared with the rack panel.
11. (eseq-groove.12) **Slim rack panel.** Picker with Library section, knobs, Extract, link to
    the tab; heatmap and rename/delete removed from the panel.

## Acceptance

- Extract from a captured Dilla-style take, quantize source, play: the take
  sounds as it did before extraction (per-hit error ≤ the slot median residual).
- Same rack, source pattern deleted, variable-reset attached: the hats sit late
  on the downbeats the way the take's hats did, and kicks land where the take's
  kicks landed.
- Rack with no active groove: scheduler output is bit-identical to today
  (existing swing/neural tests untouched).
- Offline render of a grooved rack with Random > 0 is reproducible run to run.
- Rev 2: a groove extracted on kit A, saved to the library, applied to kit B
  (different layout, roles set) puts A's snare row on B's snare; the project
  still plays it after the library file is deleted.
