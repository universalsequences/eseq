# Track Sound Binding — bare lanes, sticky sounds, and intentional arrangement editing

Status: DESIGN (rev 1, 2026-08-06). Direction confirmed with user.
Builds on: `docs/takes-and-additive-arrangement-recording-spec.md` (§16 sound binding),
`docs/unified-transport-spec.md`, Sounds S1–S3 (pool Patch/Mix entities, palette UI).

## 1. Problem

A track's device state (instrument + FX params, "the sound") is only reachable
through three owners: a scene cell's pattern, a take chunk's frozen snapshot, or
a timeline clip selection. **A track has no sound of its own.** On a track the
user deliberately emptied (all timeline clips deleted, all scene cells cleared —
the "takes-only" workflow), every subsystem invents an owner, and they all key
off `scenes.current_scene` — a hidden global that arrangement playback silently
moves to the scene of the row under the cursor.

Observed symptoms (all traced 2026-08-06; file:lines are as-of that date):

1. **Knob touch mints a clip.** Device edits resolve their write target via
   `resolve_device_value_target` (app/edit.rs:5219); on a bare lane the sound
   binding resolves to nothing, so it falls into `ensure_effective_track_pattern`
   (edit.rs:5188) → `materialize_current_scene_pattern` (state/sequencer_state/
   accessors.rs:1374), which mints a pool pattern + scene cell + sound refs in
   `current_scene` and stamps free-run offsets into every song row referencing
   that scene. Touch a knob with the cursor near a different row → a second cell
   in a second scene. Same funnel fires from preset loads, step gestures,
   p-locks, and pattern-geometry edits.
2. **Play alone resurrects clips.** The masked save-back's bare-track branch
   (state/scenes.rs:496-521) mints a cell when the track's pool is empty but the
   live grid has any active steps — and `delete_track_pattern`
   (scene_launch.rs:913) silences the lane *without blanking the live grid*, so
   ghost steps from deleted clips survive to re-materialize. The stale-lane
   guard requires `resolved.is_some()` and never protects bare lanes.
3. **Play/Pause asymmetry.** Play (`prepare_song_playback_at`,
   app/song_transport.rs:378-438) stamps `current_scene = cursor-row's scene`
   (scene_launch.rs:706) and clears non-latched override pins, but for a
   clip-less lane silences *without touching the live device mirror*
   (scene_launch.rs:774-782) — the user keeps hearing/seeing their params.
   Pause (= stop) then runs `resync_live_grid_to_current_scene`
   (scene_launch.rs:48-100), whose `restore_to` overwrites the live mirror from
   that scene's cell. **Identity moves at Play; audio/UI move at Stop.** The
   stop-time save-back is masked (lane silenced, pin cleared) so the live params
   are dropped, not persisted. Cursor moves while stopped are inert
   (`set_arrangement_cursor` is a mirror); the "cursor retunes my track" feel is
   one Play/Stop cycle behind, plus timeline-click clip-deselect re-syncing
   bindings to `current_scene`'s cell.
4. **Recorded takes rebind.** Take punch-in stamps the take's sound from
   `bound_sound_refs` (app/take_recording.rs:721-773) = rule-3 resolution = the
   `current_scene` cell (the row's scene), while the ear follows the live mirror
   (still holding the user's sound). Recording follows identity; monitoring
   follows the mirror; stop makes the mirror agree — the audible instant-switch
   — and the committed take plays back with the wrong sound. The capture stop
   arm also has no save-back (contrast song_transport.rs:462), so mid-record
   tweaks are dropped. This violates the §16 invariant *panel = live monitor
   sound = record-clone source*.
5. **Post-pause "playing" dot.** `active_effective` is pure identity
   (`override_id.or(cell)`, scenes.rs:768) with no transport check; stop clears
   the scene-silenced + take-mask suppressors, so the row-installed scene's cell
   lights.

## 2. Design

### 2.1 Model: the track sound

Each track gains a **track sound**: a persistent Patch/Mix ref pair (same shape
as a `cell_sounds` entry) owned by the track itself, not by any cell. It is the
sound a bare lane monitors, edits, and records with.

- Storage: alongside the track in the serialized project (new versioned field;
  serde default per §2.6 seeding). The refs point at ordinary pool Patch/Mix
  entities — no new entity kind.
- The track sound is **never resolved through `current_scene`** and is
  unaffected by transport, cursor position, row mirrors, and scene launches.

### 2.2 Resolution: rule 3 splits

§16 bound-source resolution becomes:

1. timeline selection (unchanged)
2. playback-audible source (unchanged)
3. **(a)** effective pattern of `current_scene` *if the cell (or override pin)
   actually resolves to a pattern*;
   **(b)** else the **track sound**.

Rule 3b replaces today's behavior where a bare lane resolves to nothing and
every consumer improvises. Consumers that change with it:

- `resolve_device_value_target`: the bare-lane `_` arm writes to the track
  sound's Patch entity. **`ensure_effective_track_pattern` is no longer called
  from device-value edits.** (Step/geometry edits still need a pattern — see
  §2.5.)
- `bound_sound_refs` (take punch-in): resolves to the track sound on a bare
  lane — the monitor sound by construction (§2.4).
- Palette badge / `palette_target_or_binding`: shows the track sound on a bare
  lane; the pool selection stops flapping on pause.

### 2.3 Sticky bare lanes

A lane the user emptied stays empty and keeps its sound:

- **No lazy materialization on silenced lanes.** The bare-track branch of
  `save_scene_snapshot_masked` (scenes.rs:496-521) is removed for lanes that are
  scene-silenced/stale. (Whether any lazy-mint remains for genuinely-new
  never-touched tracks: no — takes spec §11.1 "bare tracks get None cells
  everywhere" already made eager minting obsolete; content creation goes through
  explicit gestures, §2.5.)
- **Deletion blanks the live grid.** `delete_track_pattern` (and clearing the
  last cell of a lane) zeroes the lane's live `track_bits`/chords in addition to
  silencing, killing the ghost-step resurrection. Device/mixer state in the live
  mirror is retained (it now belongs to the track sound).
- **Stop-resync holds bare lanes.** In `resync_live_grid_to_current_scene`, a
  lane whose `current_scene` cell resolves to `None` (and has no override pin)
  is left untouched: no `restore_to`, no sound-binding release-and-refallback
  that lands on a different Patch. The live mirror *is* the track sound.
- **Save-backs flow to the track sound.** Where today's masked save-back would
  skip a bare lane (dropping live edits), device-state deltas persist into the
  track sound's Patch entity instead. In particular the **capture stop arm gains
  the missing save-back** so tweaks made while recording persist.

### 2.4 Recording

- Take punch-in stamps `sound` from the resolved binding, which on a bare lane
  is now the track sound = exactly what was monitored. §16 invariant restored:
  *panel = live monitor sound = record-clone source*.
- Per-chunk frozen snapshots still clone at punch-in (unchanged mechanism), but
  they clone the track sound's device state, not a foreign cell's.
- `select_committed_take` auto-select is unchanged; playback of the take now
  matches what the user heard.

### 2.5 Creating content on a formerly-bare track

- **Seeding rule:** when a cell/clip/take is first created on a bare lane (step
  gesture in session view, clip paint on the timeline, take commit), the new
  pattern's sound **seeds from the track sound** (clone-by-default per §3's
  layering; a shared-ref option is rejected — it would make later per-clip
  clones retroactive). This is the "set the sound, then record" workflow.
- Step/geometry gestures still materialize a pattern (they need step storage) —
  but only *explicit content gestures* do; device edits never do.
- Session mode is otherwise untouched: cells with patterns, scene launches,
  punch-in step editing all resolve through cells exactly as today, because in
  session workflows the cells exist.

### 2.6 Seeding on load / migration

Existing projects have no track-sound field. On load (serde default path), seed
each track's sound by cloning the refs of the first resolving cell
(scene-0-first scan), else mint a fresh default Patch. No project rewrite pass;
the field serializes forward on next save.

### 2.7 Identity and mirror move together

With §2.2–2.3, the Play/Stop split disappears for bare lanes: Play has nothing
to silently re-own, Stop has nothing to restore — the sound just stays. For
lanes with real clips, row changes already retune audibly at the boundary during
playback; the rule going forward is **no deferred-to-stop retuning**: any seam
that moves identity (`current_scene`, pins) without moving the mirror at the
same moment is a bug. The post-pause "playing" dot resolves itself for bare
lanes (no cell to light); the general identity-vs-transport dot semantics are
out of scope here.

## 3. Phase 2 — per-clip sound clone (user-requested workflow)

Goal: click a clip → open palette → **clone** the current sound → knob edits
affect *just that clip*, track sound untouched.

This layers on existing machinery: rule-1 selection binds the panel to the
clicked clip's sound; the palette's clone gesture (S3 relink funnel) mints a new
pool Patch and re-points the clip's sound ref at it; subsequent edits flow to
the bound source (the selected clip's new Patch). Layering:

> **track sound** (default) → **clip/cell sound** (override) → **take sound**
> (frozen per-take)

**The shared-pattern caveat (must be decided before building):** a cell pattern
is commonly referenced by many timeline regions (every row of a scene-launch
recording that resolves that scene). Cloning the *cell's* sound retunes every
region resolving through it. Two options for true per-region scope:

- **(A) Make-unique on clone** (Ableton-style): the clone gesture on a timeline
  region first clones the pattern for that region (region's override pin points
  at the new pattern), then clones the sound onto it. Simple model, no new
  state; cost = pattern duplication.
- **(B) Per-region sound override**: add optional sound refs to
  `ProjectSongTrackOverride`, resolved above the cell sound. No pattern
  duplication; cost = a fourth sound-resolution layer and palette UI for it.

Recommendation: **(A)** — it reuses the existing override-pin + relink
machinery, keeps resolution three-layered, and matches user intuition ("this
clip is now its own thing"). Revisit (B) only if pattern duplication proves
heavy in practice.

## 4. Non-goals

- No change to session-mode scene/cell semantics, scene launches, or the
  manual-override latch machinery (unified-transport spec).
- No change to take chunk storage (per-chunk snapshots stay; hoist-to-TrackTake
  remains deferred per §16).
- The identity-based "playing" dot semantics beyond bare lanes.

## 5. Build order (suggested)

1. Model + serialization: track sound field, load seeding (§2.1, §2.6).
2. Resolution rule 3b + `resolve_device_value_target` retarget (§2.2) — kills
   symptom 1.
3. Sticky bare lanes: remove lazy mint, blank-on-delete, resync hold, save-back
   retarget incl. capture stop arm (§2.3) — kills symptoms 2–3.
4. Take punch-in via track sound (§2.4) — kills symptom 4.
5. Seeding new content from track sound (§2.5).
6. Phase 2 per-clip clone (§3) after 1–5 settle.

Each step lands with regression tests pinned to the symptom it kills (e.g.
"knob touch on a bare lane creates no cell", "pause does not change the audible
sound on a bare lane", "a recorded take plays back with the monitored sound").
