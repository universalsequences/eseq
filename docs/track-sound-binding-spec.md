# Track Sound Binding — bare lanes, sticky sounds, and intentional arrangement editing

Status: DESIGN (rev 4, 2026-08-06). Direction confirmed with user.
Rev 2: rule 3a's predicate changes from *cell existence* to *cell audibility*
(§2.2.1) after symptom 6 was reproduced on a lane that is timeline-bare but
still owns session cells. Matching edits in §2.3 (resync hold, save-back).
Rev 3: takes **share** the track sound's refs by design (§2.4.1 — a later
track-sound edit retunes every take bound to it; divergence is an explicit
palette clone, user-confirmed UX). §3's "frozen per-take" layer is amended
accordingly. The reload flip-flop (symptom 7) is therefore NOT a sharing bug
but a broken invariant: a seam rebuilt the mirror without installing the
track sound (§2.8).
Rev 4 (user-designed): ownership is **view-keyed** (§2.2.2), replacing
§2.2.1's `scene_silenced` predicate. Symptom 8 showed that ANY
transport-history-derived predicate mis-targets edits made while stopped;
the durable intent signal is the view the user is standing in. Arrangement
view → the track owns the sound (rules 1/2 excepted); Seq view → pure
scene+pattern world. Adds §2.9 (view-switch seam + track-param
write-through) and §5.3 (build delta).
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
6. **Session-occupied, timeline-bare lanes reproduce 3+4 after the rev-1
   build.** (Reproduced 2026-08-06 with steps 1–5 built.) A track whose
   timeline clips were all deleted but whose *session cells still exist* is not
   "bare" under rev 1's cell-existence predicate, so every protection sits out:
   Play at a cursor row silences the lane and stamps `current_scene` without
   touching the mirror; punch-in's `bound_sound_refs` falls to rule 3a and
   stamps the *cell's* sound ("Patch N (scene)") while the ear follows the
   mirror; the stop resync's `Some(data)` arm `restore_to`s the cell over the
   mirror (the audible snap); and the masked save-back skips the lane entirely
   (stale but not cell-less), dropping mid-record tweaks. Root cause: rev 1
   keyed "bare" on whether the cell *exists*; the §2.4 invariant needs whether
   the cell is *monitored*.
7. **The reload flip-flop: committed takes retune themselves across
   transport stops.** (Reproduced 2026-08-06 with rev 2 built.) Record takes
   on a bare lane with a chosen sound — playback correct. Reopen the project:
   the first playback is correct, then one pause → play and the takes play
   the stock patch. Mechanism: the takes share the track sound's entities
   (correct, §2.4.1), but the load path rebuilt the live mirror WITHOUT
   installing the track sound into it — mirror and track sound silently
   diverged with no user gesture — and the next stop's bare-lane save-back
   (§2.3) trusted the mirror and overwrote the shared entities with stock.
   Same family: deleting a take's timeline clip audibly reverts the lane to
   the stock patch. Root cause: the §2.8 invariant was never enforced at
   those seams.
8. **Edits made while stopped target the wrong owner; track params never
   persist at all.** (Reproduced 2026-08-06 with rev 3 built.) Add a track
   (auto-creates cells + timeline clips), delete its timeline clips, select
   a preset while STOPPED, record two takes at different cursor positions →
   both end up on the stock patch. Mechanism: `scene_silenced` — rev 2/3's
   ownership predicate — is a runtime flag that only turns on when playback
   passes over the lane. The stopped-time preset therefore hit rule 3a and
   wrote the **scene cell**; the first punch-in stamped the still-stock
   track sound; the take's frozen chunks (stock) re-entered the mirror via
   the capture-stop auto-select borrow; and each stop save-back wrote
   whatever the mirror happened to hold into the shared track sound — last
   writer wins, both takes retuned. Related: the track `polyphonic` toggle
   resets on Play/cursor moves — track params live only in the mirror until
   a stop save-back, and borrows/releases/row-applies freely repaint the
   mirror in between, discarding the edit (§2.9). Root cause: intent was
   INFERRED from transport history instead of read from the user's context;
   any such predicate mis-answers "who owns this lane" for gestures made
   while stopped.

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
3. **(a)** effective pattern of `current_scene` *if that cell (or override
   pin) is the lane's **installed** source — it resolves to a pattern AND the
   lane is not scene-silenced* (§2.2.1);
   **(b)** else the **track sound**.

#### 2.2.1 The audibility predicate: "the binding follows the mirror" — SUPERSEDED by §2.2.2

> Rev 4: §2.2.1's predicate (`cell resolves && !is_scene_silenced`) is
> superseded. It was the right *invariant* (record what you monitor) built
> on the wrong *signal*: `scene_silenced` is transport-derived, so ownership
> silently changed with playback history — symptom 8. The mirror-follows
> principle survives; the discriminator moves to the view (§2.2.2).
> `scene_silenced` returns to being playback display state only.

**A scene cell owns a lane's sound only while it is actually installed in the
live mirror** (launched / playing / restored). A cell that merely *exists*
while the lane is scene-silenced has no say. Rationale: the §2.4 invariant is
*panel = live monitor sound = record-clone source*, and on a silenced lane the
monitor is the mirror, which no cell ever repainted.

`scene_silenced` is already the machine-readable form of the user's intent:
deleting a lane's timeline clips makes song playback resolve nothing for it →
the lane silences with the mirror untouched → "takes only, no scenes here."
No new per-track mode/flag is introduced; the predicate is
`cell resolves && !is_scene_silenced(track)`.

The scene workflow is preserved for free: **launching** a clip (session click,
scene launch, or the manual-latch path mid-arrangement-recording) installs the
cell's pattern *and sound* into the mirror and clears the silenced flag — the
cell genuinely becomes the monitor, so rules 2/3a bind to it, recording stamps
it, and everything agrees. Launch is exactly the gesture that says "scenes
again, please."

#### 2.2.2 View-keyed ownership (rev 4, user-designed)

**The view the user is standing in IS the intent signal.** No inference from
deletion history, playback history, cursor position, or cell existence — all
of those mis-answer "who owns this lane" for gestures made while stopped
(symptom 8) or produce answers that flip with transport state (symptoms 3–7).

Rule 3 becomes:

- **Arrangement view → the TRACK owns the sound.** Panel, device edits,
  preset loads, track-param toggles, punch-in stamps, and bare-lane
  save-backs all target the **track sound** — stopped or playing, wherever
  the cursor sits, whatever cells exist. Exactly two things outrank it,
  and they are rules 1 and 2 unchanged:
  1. an explicit **selection** (a clicked clip; incl. a clip whose sound was
     diverged via the palette — §3);
  2. an **audibly playing source** on the lane (a pattern/take clip under
     the playhead, or a scene/clip launch made from arrangement view — the
     live-set flow — which takes the lane via rule 2 for as long as it
     sounds; boundary retunes as today).
- **Seq view → pure scene+pattern world.** Classic rule 3a: the current
  scene's cell owns the lane; edits and save-backs flow to cells; scene
  launches install cells into the mirror. The track sound is **dormant** —
  never a read or write target in this view. It still exists; arrangement
  view picks it back up on switch (§2.9).

The user's canonical scenario, decided by this rule: keep the first 8 bars
of pattern clips, delete everything after, park the cursor past bar 8 while
STOPPED, select a preset for the upcoming take → arrangement view → the
preset lands on the track sound, the monitor plays it, punch-in stamps it,
and the first 8 bars still retune the lane while their clips audibly play.
(Under rev 2/3 this preset went to a scene cell, because nothing had
silenced the lane yet. Under the earlier "delete-gesture marks the track"
idea it ALSO failed, because the user never deleted the last clip. Only the
view reads the intent correctly.)

Session cells while in arrangement view are inert-but-visible: unlaunched
clips that re-engage through an explicit launch (rule 2) or by switching to
Seq view. This matches the rev-2 dot/unlaunched semantics, which survive.

Rule 3b replaces today's behavior where a bare lane resolves to nothing and
every consumer improvises. Consumers that change with it (rev 4: "bare lane"
now reads "track-owned lane" — any arrangement-view lane rules 1/2 don't
claim):

- `resolve_device_value_target`: the bare-lane `_` arm writes to the track
  sound's Patch entity. **`ensure_effective_track_pattern` is no longer called
  from device-value edits.** (Step/geometry edits still need a pattern — see
  §2.5.)
- `bound_sound_refs` (take punch-in): resolves to the track sound on a bare
  lane — the monitor sound by construction (§2.4).
- Palette badge / `palette_target_or_binding`: shows the track sound on a bare
  lane; the pool selection stops flapping on pause.

### 2.3 Sticky bare lanes

> Rev 4: where this section says "bare" or "scene-silenced", read
> "track-owned per §2.2.2" (arrangement context, rules 1/2 unclaimed). The
> behaviors below survive; only the predicate moves.

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
- **Stop-resync holds bare AND silenced lanes.** In
  `resync_live_grid_to_current_scene`, a lane is held — no `restore_to`, no
  sound-binding release-and-refallback that lands on a different Patch —
  when its `current_scene` cell fails the §2.2.1 predicate: the cell resolves
  to `None` (and no override pin), **or the lane is scene-silenced** (rev 2).
  The live mirror *is* the track sound. A held cell stays *unlaunched* after
  Stop (dot off, like an un-clicked Ableton session clip); it re-engages only
  when explicitly launched. (Latched lanes keep their existing carve-out.)
- **Save-backs flow to the track sound.** Where today's masked save-back would
  skip a bare lane (dropping live edits), device-state deltas persist into the
  track sound's Patch entity instead — and (rev 2) the same applies to a
  **scene-silenced lane whose cell still exists**: the cell is not installed,
  so its entities are not touched; the mirror's device half persists into the
  track sound. This keeps the track sound converged with what the user hears,
  so the punch-in stamp and the monitor cannot drift. In particular the
  **capture stop arm gains the missing save-back** so tweaks made while
  recording persist. (Latched lanes still self-write via their pin, never the
  track sound.)

### 2.4 Recording

- Take punch-in stamps `sound` from the resolved binding, which on a bare lane
  is now the track sound = exactly what was monitored. §16 invariant restored:
  *panel = live monitor sound = record-clone source*.
- Per-chunk frozen snapshots still clone at punch-in (unchanged mechanism), but
  they clone the track sound's device state, not a foreign cell's.
- `select_committed_take` auto-select is unchanged; playback of the take now
  matches what the user heard.

#### 2.4.1 Takes SHARE the track sound (rev 3, user-confirmed UX)

Punch-in **shares** the track sound's refs (§17.3 "record → share"), it does
not fork them. Consequences, all intended:

- Record 3 takes with the track sound, then edit the track sound (no take
  selected): **all 3 retune together** — same model as clips sharing a pool
  Patch.
- Editing a *selected* take edits the shared entities too, so it also retunes
  its siblings and the track sound. To make one take/clip its own thing, the
  user **explicitly forks via the sounds palette** (the S3 clone/relink
  funnel) and then edits — sharing by default, divergence by gesture.
- A fork-at-punch-in alternative was considered and REJECTED: it would freeze
  every take at record time, so track-sound edits would never reach existing
  takes and editing one take would silently diverge it from siblings — a
  different (worse) UX than the palette's explicit-clone model.

This is safe **only** while the §2.8 invariant holds: the save-back writes
the mirror into the shared entities at every stop, so any seam where the
mirror stops being the track sound retroactively retunes committed takes
(symptom 7).

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

### 2.8 The mirror invariant (rev 3): on a bare lane, the mirror IS the track sound

The §2.3 save-back makes the mirror authoritative over the track sound's
entities at every stop. That is only sound (pun intended) if **every seam
that rebuilds the live mirror for a bare/silenced lane installs the track
sound into it** — the mirror and the track sound may never diverge except
through a live user edit (which the next save-back then legitimately
persists, retuning shared takes per §2.4.1).

Seam inventory (all traced 2026-08-06; enforce at each):

- **Project load** (symptom 7's CONFIRMED breaker):
  `restore_current_pattern_from_repository` (accessors.rs:1605) restores
  resolving lanes from their cells but its bare-lane arm only sets
  `scene_silenced` — the mirror keeps fresh-track defaults from the
  AddTrack/AddEffect phases, while the track sound holds the file's real
  entities (relinked in the pools only). Fix: the bare arm restores the
  carrier's device half into the mirror. (`restore_track_sound_to_mirror` is
  called from exactly one site today — `after_sound_repoint` — and is NOT on
  the load path.)
- **Borrow release** (the delete-clip revert's CONFIRMED seam):
  `release_borrowed_lanes` (song_playback.rs:573) restores a released lane
  from `effective_track_pattern` — `None` on a bare lane, so the borrowed
  take's device state silently stays in the mirror. Fix: fall back to the
  track-sound carrier (and per §2.2.1, an *uninstalled* cell must not be the
  restore source either).
- **Take punch-in chunk template** (the "select the take → stock patch"
  half of the delete-clip repro): `take_record_note`'s template falls from
  `bound_read_pattern` (now `None` on a bare/silenced lane) to
  `effective_track_pattern` to `new_default` — so the take's frozen per-chunk
  device snapshots hold the STOCK patch, violating §2.4 ("they clone the
  track sound's device state"). Selecting the take borrows that default into
  the mirror; the next save-back poisons the shared entities. Fix: the
  template falls back to the track-sound carrier before the default.
- **Stop resync** — already held (§2.3 rev 2): silenced lanes are skipped, so
  the mirror stays the track sound.
- **Palette repoint** — already enforced (`after_sound_repoint` restores the
  carrier to the mirror on bare lanes).
- **Entity pruning** — verified safe: the carrier is an ordinary pool
  pattern, so `referenced_sounds`/`prune_unreferenced_sounds` count it as a
  referent; deleting takes/clips cannot collect entities the carrier names.
- **Track add** — trivially consistent (fresh mirror, freshly seeded default
  track sound).

Litmus test for any future seam: after the seam runs, would an immediate
stop-time save-back write anything into the track sound that the user never
heard or dialed in? If yes, the seam is broken.

**Borrow-claim rule (post-rev-4, fourth disease bug — the palette-clone
note clobber): an arrangement-context borrow IS a rule-1/2 claim for the
save masks.** A borrowed lane (selected take/clip) was in no mask at all —
not latched, not track-owned (excluded *because* borrowed) — so a save-back
stored the live grid's arrangement step content into the session cell's
notes. `arrangement_borrowed_lane_mask()` now folds into
`stale_live_lane_mask()` and `masked_save_masks()`, which is the single
mask-derivation seam — never assemble the mask triple by hand.

**Claim-end rule (post-rev-4, second poisoning fix): every path that ends a
rule-1/2 claim in arrangement context reinstalls the track sound into the
mirror.** A claimed lane's mirror legitimately holds foreign state (the
borrow's or the launch's devices); when the claim ends, that state stays
behind unless the owner is reinstalled — and the next save-back, seeing an
unclaimed track-owned lane, persists it into the shared entities (the
recorded-clip-launch poisoning). The borrow release has done this since
rev 3 (`release_borrowed_lanes` → carrier fallback); the latch-release
sites (capture stop, back-to-song both branches, per-track back-to-song,
capture cancel) do it via
`restore_track_sounds_to_mirror_masked(released_latch)` — self-guarding
(no-op in Seq context, spares re-claimed lanes). The SongPlayback stop arm
needs nothing: its latch survives, so the lane stays masked.

**Ordering rule (post-rev-4, first poisoning fix): a snapshot must be saved
under the masks it was captured under.** `capture_current_pattern_snapshot`
substitutes a BORROWED lane's device half with its cell's; if
`release_bound_device_state` runs before the save-mask derivation, the
just-released lane counts as track-owned and the cell's stock device state
is written into the shared track-sound entities (the first-Play-after-
recording poisoning). Every capture→release→save trio reads
`masked_save_masks()` before the release.

### 2.9 View switching and write-through (rev 4)

**The view switch is a first-class mirror seam** — the one place ownership
legitimately changes wholesale, so it must move edits out and owners in, in
that order:

1. **Leaving a view: save back to that view's owners.** Leaving arrangement
   view persists track-owned lanes' mirror state into their track sounds
   (rule-1/2-claimed lanes self-write per their existing carve-outs).
   *(As built: the leave save-back runs only arrangement→Seq. Seq-context
   edits already write through to the cell at edit time — device values, and
   since rev 4 track params — so a blind mirror→cell save on the way out of
   Seq adds no durability and could only clobber a cell from a mirror some
   borrow desynced; running it both ways broke two take_edit pins for
   exactly that reason.)*
2. **Entering a view: install the new owners into the mirror.** Entering Seq
   view installs the current scene's cells (an ordinary resync/launch —
   restores rev-1 session behavior wholesale). Entering arrangement view
   installs the track sound on every lane rules 1/2 don't claim.

**Track-param write-through** (independent of views, fixes the poly reset):
edits to mirror-resident track params (`polyphonic`, timebase, mute-group,
etc.) persist to the owning entity **at edit time**, exactly as device
values already do through `resolve_device_value_target` — never parked in
the mirror awaiting a stop save-back that a borrow/release/row-apply may
preempt. The stop save-back remains as a safety net, not the mechanism.

With §2.2.2 + §2.9, the §2.8 invariant simplifies to: **the mirror always
holds the current owner per resolution**, and the seam inventory's fixes
(load, borrow-release, punch-in template) survive re-keyed — each installs
or clones "the owner", which in arrangement view is the track sound.

## 3. Phase 2 — per-clip sound clone (user-requested workflow)

Goal: click a clip → open palette → **clone** the current sound → knob edits
affect *just that clip*, track sound untouched.

This layers on existing machinery: rule-1 selection binds the panel to the
clicked clip's sound; the palette's clone gesture (S3 relink funnel) mints a new
pool Patch and re-points the clip's sound ref at it; subsequent edits flow to
the bound source (the selected clip's new Patch). Layering:

> **track sound** (default) → **clip/cell sound** (override) → **take sound**
> (shared with its recording source by default — rev 3, §2.4.1; "its own
> thing" only after an explicit palette clone)

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

### 5.1 Rev 2 delta (steps 1–5 built; this fixes symptom 6)

One predicate swap ("cell exists" → "cell installed", §2.2.1) applied at three
seams, plus tests:

1. **Resolution** — `bound_sound_refs` / `track_sound_binding`'s rule-3
   candidate: a scene-silenced lane skips rule 3a and falls to the track sound.
2. **Stop resync** — `resync_live_grid_to_current_scene`: hold silenced lanes
   (mirror untouched, cell left unlaunched) instead of `restore_to`.
3. **Masked save-back** — `save_scene_snapshot_masked`: a silenced lane with a
   resolving cell persists its device half into the track sound (cell entities
   untouched); requires the caller to pass a silenced-lane mask (the flag lives
   on `SequencerState`, not `ProjectScenes`).

Regression tests pinned to symptom 6: "punch-in on a silenced lane with
session cells stamps the track sound", "stop after recording on such a lane
does not retune the mirror and leaves the cell unlaunched", "mid-record device
tweaks on such a lane persist into the track sound", and the preserved scene
path: "launching the cell re-installs its sound and rebinds rules 2/3a to it".

### 5.2 Rev 3 delta (fixes symptom 7)

Three seam fixes from the §2.8 inventory, plus tests:

1. `restore_current_pattern_from_repository` bare arm restores the carrier's
   device half into the mirror (load seam).
2. `release_borrowed_lanes` falls back to the carrier when the lane has no
   *installed* cell (borrow-release seam).
3. `take_record_note`'s chunk template falls back to the carrier before
   `new_default` (frozen snapshots clone the track sound, §2.4).

Tests: "after a repository restore, a bare lane's mirror holds the track
sound", "releasing a bound take on a bare lane restores the track sound to
the mirror", "a take recorded on a bare lane freezes the track sound's device
state into its chunks", plus the §2.4.1 sharing pin
(`editing_the_track_sound_retunes_the_takes_that_share_it`, already in).

### 5.3 Rev 4 delta (view-keyed ownership; fixes symptom 8)

Revs 2–3 are built; this re-keys their predicate from `scene_silenced` to
the view and adds the view-switch seam. Suggested order:

1. **Plumb the view bit.** Ownership consumers live on both `App`
   (`arrangement_view_visible` already exists) and `SequencerState`
   (save-back masks, resync, `mirror_device_pattern_id`), which cannot see
   the App. Give `SequencerState` a control-side "arrangement context" flag
   the App writes on view switch (same shape as `song_take_lane_mask`).
2. **Re-key rule 3** in `track_sound_binding` / `bound_sound_refs` /
   `mirror_device_pattern_id` / `resolve_device_value_target` / the preset
   path: arrangement context → track sound (rules 1/2 excepted); Seq context
   → effective cell. Remove the rev-2 `is_scene_silenced` gates; the flag
   reverts to display-only.
3. **Re-key the masked save-back**: replace `silenced_mask` with a
   track-owned mask derived from the context flag + rule-1/2 claims. Seq
   context saves to cells exactly as rev 1.
4. **View-switch seam** (§2.9): save-back on leave, owner install on enter,
   both directions. The rev-3 seam fixes (load, borrow-release, punch-in
   template) stay, re-keyed to "the owner".
5. **Track-param write-through** (§2.9) — independently testable; kills the
   poly reset.
6. **Re-pin tests**: the 8-bar scenario ("preset while stopped past the
   clips lands on the track sound and the take plays it back"), a
   view-switch roundtrip ("edit in arrangement view → switch to Seq and back
   → edit persisted, owners correctly installed"), poly persistence across
   Play/cursor moves, and re-key the rev-2/3 symptom pins (the
   `apply_song_row_latched` silenced-lane setups become arrangement-context
   setups).

Behavior notes: the rev-2 "stop holds silenced lanes" test becomes
view-dependent (arrangement view: stop HOLDS track-owned lanes — the mirror
already is the track sound per §2.8, so no repaint, which also protects
unsaved live edits; Seq view: stop resyncs cells — the rev-1 behavior
returns there). As built, the state-side "rule 1/2 claimed" derivation for
save-back masks is `latched ∪ borrowed` (`sound_binding_borrowed_mask`):
a selected clip's borrow marks the lane so its edits never leak into the
track sound. KNOWN GAP (AddTrack seeding): a freshly added track's carrier
holds a stock default Patch until a track-owned save-back converges it —
§2.6 seeds on LOAD only; the AddTrack path should seed the carrier from the
track's instrument descriptor once it exists. Takes recorded under revs 1–3 may carry stock frozen chunks
(symptom 8's third mechanism); §2.9's install rules stop them from
poisoning the mirror's owners, but their frozen snapshots stay stock until
re-recorded or palette-relinked.
