# Key Locks (Per-Note Parameter Locks) Spec

Status: draft / design
Author: design pass, 2026-07-07
Related: `docs/racks-spec.md` (ByPitch routing is a sibling idea), recent p-lock UX work (`8d065f39`)

## 1. Goal

Let a patch store **parameter overrides keyed on pitch**: playing C4 sets
`cutoff=3000`, playing D4 sets `detune=-40`, etc. The direct inspiration is
**AFX Mode on the Novation Bass Station II** (the Aphex Twin firmware feature),
where each key carries its own parameter overlay — used to spread whole drum
kits or timbre variations across the keyboard.

This makes p-locks *playable*: the keyboard (live or sequenced) becomes a
selector of sound variations, and transposing a pattern reveals different
sounds.

**Framing / naming.** Key locks sit conceptually beside mods:

- **mods** — a parameter as a function of *time* (lfo / env / rand / drift)
- **key locks** — a parameter as a function of *pitch*

Both are properties of the **patch**, not the sequence. Both save with the
preset. Neither lives in the track/sequencer data.

## 2. Locked decisions

1. **Key locks are patch data, not pattern data.** Stored with the instrument's
   sound state and serialized into `InstrumentPreset`. They survive pattern
   switches and apply identically to live playing and sequenced notes. (AFX
   Mode precedent: overlays are stored with the patch.) Per-pattern overrides
   are explicitly out of scope for v1.
2. **Resolve precedence, per-param:** patch default → **key lock** → **step
   p-lock** → modulation on top. The step p-lock is the more specific,
   deliberate gesture and wins on conflict; conflicts resolve *per parameter*
   (a step locking only `cutoff` must not suppress the key's `detune` lock).
   Mods modulate around whatever base value survives the override chain.
3. **Locks key on the *sounding* pitch** (post-transpose, post-scale, after
   midi-fx). This is what makes transposing a pattern expressive rather than
   confusing, and matches AFX behavior. The lock lookup happens at the same
   point where the final note value is known.
4. **Per-voice application.** All instrument parameter knobs are per-voice, so
   two simultaneous notes with different key locks each sound their own
   values. No last-note-priority policy is needed; every lockable synth param
   is key-lockable.
5. **Editing lives in a new instrument-panel tab** (`synth | mods | keys`),
   not in the expanded-track sequencer view. Rationale: it's patch config, and
   the mods tab already established the layout + gesture grammar (see §5).

## 3. Data model

Sparse map per instrument slot, mirroring the shape of step p-locks but keyed
on MIDI note instead of step index:

```
key_locks: note (u8 / i16) → { param_idx → f32 }
```

### Runtime state

Lives on the instrument slot state alongside `plocks` (the step-indexed
`Vec<Vec<Option<f32>>>` on the instrument slot — see
`crates/sequencer/src/project.rs:411` and `sequencer/state.rs`). Suggested
shape: `BTreeMap<u8, Vec<Option<f32>>>` (sparse over notes, dense over params,
so the resolve loop matches `resolve_instrument_plocks`). Reuse the
param-identity guard (`plock_param_ids` / `plock_identity_matches`,
`scheduler.rs:446`) so stale locks are dropped when the engine's param layout
changes — key locks need the same protection as step p-locks.

Mod params (`raw_idx >= MOD_PARAM_BASE`, targeting the voice modulator) are
addressable through the same plumbing; v1 may key-lock them too if it falls
out for free, otherwise restrict to synth-target params.

### Preset serialization

`InstrumentPreset` (`crates/sequencer/src/lisp_host.rs:1762`) gains an
optional field, `#[serde(default)]` so existing banks load unchanged:

```rust
pub struct InstrumentPreset {
    pub id: String,
    pub name: String,
    pub base_note_offset: f32,
    pub params: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_locks: BTreeMap<u8, BTreeMap<String, f32>>,  // note → param name → value
}
```

Param *names* (not indices) in the serialized form, same as `params`, so
presets survive param reordering in dsp.lisp. This is what unlocks shippable
AFX-style kits: a preset can be "808 kit across the keyboard."

Project save/load: key locks ride wherever the rest of the live patch state
rides (`ProjectTrackSoundState` / instrument slot snapshot in `project.rs`),
same versioning approach (`serde(default)`).

### Host feature, not instrument feature

Key locks are implemented entirely in the host (storage, resolve, UI), exactly
like step p-locks. Instruments' `dsp.lisp` / `ui.lisp` need zero awareness.

## 4. Engine: resolve + apply

### The per-note wrinkle (the one real engine change)

Step p-locks are resolved **per step**: `resolve_instrument_plocks`
(`scheduler.rs:799`) produces one `ScheduledInstrumentParams` set per trigger,
carried on `ResolvedTrigger` / applied via
`dispatch_instrument_params_to_active_voices` (`audio.rs:2680`).

Key locks are **per note**: a chord on one step can hit different locks per
note. One shared param set per trigger is not enough. Two options:

- **(a) Resolve at voice-assignment time (recommended).** At the point in
  `dispatch_scheduled_step` where each note is assigned a voice, look up the
  key-lock map for that note and apply its params to *that voice*, before
  applying the trigger's step-p-lock param set on top (per-param precedence:
  only apply a key-locked param if the step p-lock didn't set that param —
  or simpler: apply key locks first, then step p-locks overwrite). This
  requires the key-lock map to be visible from the audio-side snapshot, same
  as `instrument_slot` already is.
- **(b) Resolve scheduler-side into per-note param sets** carried on the
  trigger (e.g. `[ScheduledInstrumentParams; MAX_VOICES]` parallel to
  `notes`). Keeps the audio side dumb but fattens the event payload.

(a) is preferred: the sounding pitch is definitely final there, it covers
live-played notes through the same voice-trigger path, and the event queue
stays unchanged.

### Live input

Live keyboard/MIDI note-ons must hit the same lookup. If live notes flow
through the same voice-trigger dispatch, (a) covers this for free — verify
that path.

### Voice params vs. lingering voices

Applying at voice-trigger time means a lock affects only the newly triggered
voice; released-but-ringing voices keep their values. That is the correct
per-voice semantic (matches how step p-locks behave with
`dispatch_instrument_params_to_active_voices` scoped per trigger).

## 5. UI

### 5.1 The `keys` tab

New tab in the instrument panel header: **`synth | mods | keys`**. The mods
tab already established the layout: config pane on the left, **the synth's
own knob panel stays visible on the right**. The keys tab reuses this:

```
┌─ membrane-tab   synth | mods | [keys] ──────────────────────────┐
│ ┌──────────────────────────┐  ┌───────────────────────────────┐ │
│ │  piano keyboard          │  │  synth knob panel (unchanged  │ │
│ │  ●        ●     ●        │  │  layout, knobs write to the   │ │
│ │ ┌┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┬┐  │  │  selected key(s))             │ │
│ │ ││█││││█││││││█│││││││   │  │                               │ │
│ │ └┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┴┘  │  │                               │ │
│ │  octave ◀ ▶   [clear key]│  │                               │ │
│ └──────────────────────────┘  └───────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

**Gesture grammar (the core consistency win):** left pane selects a *scope*,
right pane's knobs *write into that scope* — identical to how the mods tab
works and to select-then-turn on steps. A user who knows step p-locks or mods
learns key locks instantly.

- **Select a key** → it arms like a held step. Turning any knob on the right
  writes a key lock for that key.
- **Multi-select** (shift-click / drag range) → knob writes go to all selected
  keys. This *is* the zone feature; no first-class zone abstraction in v1.
- **Audition on select**: selecting a key sounds it (respecting its locks) so
  you hear the lock while turning. Toggleable.
- **Clear**: same gesture as clearing a step p-lock (post-`8d065f39` UX),
  per-param; plus a "clear key" affordance for wiping a whole key.
- **Lock indicators on the keyboard**: any key with ≥1 lock gets an
  always-visible dot/tint above it, so the kit layout is scannable without
  selecting each key. Hover: param count / names.

### 5.2 Knob display in the keys tab

When a key is selected, knobs show that key's **effective values**, and locked
params are visually unmistakable:

- **Locked value renders in a distinct lock color** (the value text and arc
  fill), consistent with the step-p-lock affordance.
- **A dedicated backplate/background behind the value** marks "this is a key
  lock" — required because many synth panels already use multi-colored knob
  arcs per section (MASK/STROKE/HEAD/BODY colors), so color alone on the arc
  is ambiguous. The backplate is the unambiguous signal; the lock color rides
  on top of it.
- Unlocked params show the patch base value in normal styling (optionally
  slightly dimmed to reinforce "you are in an override view").
- The patch default remains discoverable (ghost tick on the arc, or the
  LOCK/DEF pairing from the inspector).
- **No key selected**: knobs show base values; turning a knob does nothing (or
  is disabled) — the keys tab never silently edits the base patch.
- **Multiple keys selected with differing values**: knob shows a
  mixed-state indicator; turning writes the new value to all selected keys.

**Write-scope rule (avoid cross-tab surprises):** knob writes are scoped by
the *active tab*. Synth tab always edits the patch base; keys tab always edits
the key selection. Key selection persists visually across tab switches but
never redirects synth-tab edits.

### 5.3 Inspector generalization

The right-hand p-lock inspector (PARAM / LOCK / DEF table) is currently "a
view on the current step." Redefine its contract to "a view on the current
**selection**" — step *or* key. Selecting C4 in the keys tab lists C4's locks
in the same table, editable the same way. Zero new UI; resolves the conceptual
overload. (When both a step and a key are notionally in play, the inspector
shows whichever was selected last.)

### 5.4 Keyboard widget

A host-level piano-keyboard widget (a few octaves visible, octave scroll).
`piano_roll.rs` likely has reusable key-drawing geometry. Range should cover
the instrument's playable range; keys outside any range constraint render but
are selectable regardless (locks on unplayed keys are harmless).

## 6. v1 scope

**In:**
- Sparse `note → {param → value}` map on the instrument slot, per-voice apply
  at voice-trigger time (option (a) in §4), sounding-pitch keyed.
- Precedence: default < key lock < step p-lock (per-param), mods on top.
- `keys` tab with select-then-turn, multi-select, audition, clear,
  lock-dot indicators, lock-color + backplate knob styling, tab-scoped writes.
- Inspector generalized to selection (step or key).
- Preset serialization (`key_locks` on `InstrumentPreset`, name-keyed,
  `serde(default)`), project save/load, param-identity guarding.
- Works for both sequenced and live-played notes.

**Out (deliberately, but design the storage so they slot in):**
- **Interpolation between locked keys** — the big v2 multiplier: a per-param
  toggle that interpolates unlocked keys between locked neighbors, turning key
  locks into arbitrary keytracking curves. Falls out if locks stay sparse and
  resolution happens at trigger time (interpolation is just a different
  resolve function over the same map).
- **Per-param lane view** — pick one param, see its value across all keys as
  a slider row over the keyboard (the expanded-track lane paradigm applied to
  pitch). Natural companion to interpolation for drawing curves.
- **Per-pattern key-lock overrides.**
- **First-class zones** (range multi-select covers the use case).
- Key-locking *effect* params (per-voice guarantee doesn't hold there).

## 7. Open questions

1. **Mod params in v1?** They flow through the same param plumbing
   (`MOD_PARAM_BASE` target split in `resolve_instrument_plocks`) — include if
   free, otherwise defer.
2. **Live-input path**: confirm live note-ons reach the same voice-trigger
   dispatch so option (a) covers them without a second lookup site.
3. **Note identity for lookup**: exact spot where sounding pitch is final
   (after `base_note_offset`, xpose lane, scale, midi-fx) — the lookup must sit
   after all of them.
4. **Sampler tracks**: out of scope (this spec is custom instruments), but the
   data model wouldn't preclude extending to sampler per-voice params later.
