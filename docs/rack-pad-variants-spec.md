# Rack Pad Variants — several pads per member track, each a saved parameter set

Status: rev 1, 2026-11-11 — design agreed in conversation, implementation in
progress. Epic `eseq-ldh3` (slices .1–.4, §10).
Related: `docs/drum-rack-v2-spec.md` (pads → member tracks),
`docs/key-locks-spec.md` (per-note locks, the closest existing feature),
`docs/jaki-plock-spec.md` (per-hit named params, which routes reuse).

## 1. Goal

A drum rack today maps each pad to exactly one member track, and every member
has exactly one pad. With trained latent kicks/snares, one instrument covers
hundreds of good sounds; what a kit needs is **many pads that play the same
member track with different saved parameter values** — "808 Clap" on C#1,
"808 Clap · tight" on D4, "808 Clap · long" on D#4.

A **variant pad** is a pad that plays its member track with a set of parameter
values layered over the patch. Everything that already addresses pads (the pad
grid, the armed-rack keyboard, recording, generators routed into the rack)
addresses variants the same way.

## 2. Locked decisions

1. **The data lives on the rack pad, not in the instrument's key locks.** A pad
   fires its member at base pitch, so every pad of one member sounds the same
   note: key locks (looked up by sounding pitch) cannot tell pads apart. And a
   variant belongs to the kit, not to the instrument preset.
2. **Key locks keep working unchanged.** A rack is sometimes just a group with
   a poly synth in it that is armed and played directly; that track's `keys`
   tab and its key locks are untouched.
3. **Precedence, per parameter:** patch default → key lock → **pad variant** →
   step p-lock → mods. A pad is a deliberate choice of sound and beats the
   patch's general per-note setting; a step p-lock is more specific still.
4. **Variants are edited in a new `pads` tab** on the member's instrument
   panel (`synth | mods | keys | pads`), shown only for rack members. It draws
   the rack's pad grid; this member's pads are highlighted, the rest dimmed.
5. **One sequencer row per member track, as now.** A variant is a property of
   a *step*, not a track: pattern length, timebase and every other track
   property stay per member. A step stores which variant it plays; its fill
   colour is the variant's colour, the same colour its pad cell shows.
6. **Steps reference a variant by a stable id, not by note.** Pads move
   (nudge, drag, swap); the step must keep playing the same variant. Editing a
   variant's values changes every step that uses it (reference, not copy).
7. **Generator routes list variants.** Every route menu that can target a
   rack member (Jaki rows, graph/neural sequencers, rack-owned or
   project-owned) lists each variant as its own entry, e.g.
   `2 Latent Kick · D4 tight`. A variant route is the member track plus the
   variant id on each hit.

## 3. Data model

### 3.1 Project

`ProjectRackPad` gains three fields (all `serde(default)`, so old projects
load as plain pads):

```rust
pub struct ProjectRackPad {
    pub pad_note: i32,
    pub member: usize,
    pub role: Option<PadRole>,
    /// 0 = the member's plain pad; 1.. = a variant, unique per member.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub variant: u32,
    /// Display name ("tight"); `None` shows the pad note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Instrument param name -> value, in the param's own units. Empty on a
    /// plain pad.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, f32>,
}
```

`ProjectRackPad` stops being `Copy`.

Invariants (replacing "a member backs at most one pad"):

- Every member has exactly one plain pad (`variant == 0`); it may have any
  number of variant pads (`variant >= 1`).
- `(member, variant)` is unique within a rack. Variant ids are allocated as
  `max(existing for member) + 1` and never reused while a pad holds them.
- `pad_note` stays unique within the rack; the pad-note domain is unchanged.
- Deleting a member deletes all its pads. Removing a variant pad leaves its
  steps pointing at a missing id; they play as the plain pad (§4.3).
- Values are keyed by **param name**, like key-lock presets, so they survive
  param reordering in `dsp.lisp`; a name the instrument no longer has is kept
  but ignored, so swapping back revives it.
- Choke groups stay per pad in the project but are *published per member
  track* (the runtime choke key is per track): a member's choke group is its
  plain pad's. The choke selector on a variant pad edits the plain pad's.

### 3.2 Step

`StepParam::Variant = 12`: default `0` (plain pad), range `0..=255`,
increment `1`. Because it is an ordinary step param it inherits storage,
serialization, undo, copy/paste, the step editor lanes, live printing and
generator `step-param:variant` per-hit params for free. `ResolvedStep` gains
`variant: u32`.

### 3.3 Runtime

`App::publish_rack_choke_runtime` (already the funnel for every rack-topology
and pad edit) also publishes, per member track, its variant table:
`Vec<RackVariantRuntime { track, variant, values: Vec<(String, f32)> }>` into
`SequencerState`. The scheduler snapshot resolves the names against the
track's instrument descriptor at capture into
`SequencerTrackSnapshot::pad_variants: Vec<PadVariantOverlay>`
(`variant`, then `(param_idx, ParamNodeId, value)` triples). Resolving at
capture means an instrument swap re-resolves without a republish.

## 4. Engine

### 4.1 Sequenced hits

`fire_resolved` gets the step's `resolved.variant`. Right after the per-voice
key-lock merge (`key_locked_live_instrument_params`) it applies the overlay
for `(track, variant)` from the scheduler snapshot: every entry whose
`ParamNodeId` still matches the live slot and that has **no valid step
p-lock** on this step is upserted into the voice's params. The voice-param
fingerprint is computed from the merged set, so a voice that last played a
variant is re-dispatched when the next hit is plain. No event-size change.

### 4.2 Live hits

`KeyboardTrigger` gains `variant: u32`. The armed-rack lookup returns
`(track, variant)` for the pad under the key; the live voice path applies the
overlay the same way (no step, so no step-p-lock skip). Roll mode's
`RollCommand::NoteOn` carries the variant too.

### 4.3 Missing variants

A step or hit naming a variant the member has no pad for plays the plain pad.
No error: deleting a variant must never silence a pattern.

### 4.4 Scope

v1 covers DGen (custom) instruments, which is what the latent drums are.
Sampler tracks' host params (`ScheduledSamplerParams`) and rack-type tracks
are follow-ups (§9).

## 5. Recording

A hit from an armed rack on a variant pad writes the note into the member
track as today and sets the step's `Variant` to the pad's id; a plain pad
writes `0`. One step holds one variant: if two variants of the same member
land on the same step, the last hit wins.

## 6. UI

### 6.1 `pads` tab (member instrument panel)

- Shown only when the panel's track is a rack member; `synth | mods | keys`
  are unchanged.
- Draws the rack's pad grid at the rack panel's current page (same paging and
  octave map as the rack panel). This member's pads are bright, other members'
  pads are dimmed and not editable here.
- **Click one of this member's pads** to focus it. With a variant focused,
  turning a knob writes the variant's value instead of the patch default (the
  keys tab's gesture). Locked knobs show the key-lock-style indicator.
  Focusing the plain pad edits the patch as usual.
- **Click an empty cell** to create a new variant of this member on that
  note, seeded empty (it sounds like the plain pad until a knob is turned).
- Focused-variant header: name field, colour chip, "clear values", "delete
  variant".
- A clicked pad auditions the member with that variant.

### 6.2 Rack panel

- Variant pad cells show the member name plus the variant name (or note) and
  the variant colour.
- Double-clicking a pad selects the member track and opens its `pads` tab with
  that pad focused, so the pads never disappear.

### 6.3 Sequencer

- Step fill colour = variant colour (`VARIANT_PALETTE[variant % 6]`), shared
  with the pad cell. Plain steps look as they do now. A step's own p-locks
  keep their usual marker on top.
- The step editor lists a **Variant** lane with the variant's name.
- With a variant focused in the `pads` tab, toggling a step on paints that
  variant onto it.

## 7. Routing from generators

- **Track list:** the kinds API exposes each rack member's variants
  (`t.variants`: id, note, name). Route menus append one entry per variant
  after the member: `2 Latent Kick · D4 tight` (`· D4` when unnamed).
- **Jaki:** a row stores `:variant` beside `:route`. Emission adds
  `"step-param:variant" v` to the hit's `:params` (jaki-plock-spec §3), which
  the scheduler already folds into the hit's step fields at landing. A row's
  own `(plock "step-param:variant" …)` still wins, so a row can sequence
  variants per hit.
- **Graph / neural sequencers:** node routes gain an optional `route_variant`
  stamped onto the node's emissions the same way.
- Labels round-trip by text like today's route menus; a pick maps back to
  `(track, variant)`.

## 8. Kits

Kit export/import carries pads verbatim, so variants, names and values travel
with no extra format work; an old kit loads as plain pads.

## 9. Follow-ups (not v1)

- Variant values for sampler host params and rack-type members.
- Effect-chain params per variant (instrument params only in v1).
- Turning a key lock into a pad variant ("spread key locks onto pads").
- Variant morphing / interpolation between two variants.

## 10. Slices

1. **Model + engine:** pad fields + invariants + load repair, `StepParam::Variant`,
   runtime publish, snapshot resolution, sequenced + live overlay, tests.
2. **Host commands:** add/remove/rename variant, set/clear a variant value,
   variant-aware armed-rack routing and recording, undo.
3. **`pads` tab + rack panel + step colouring/paint.**
4. **Routes:** kinds API `variants`, Jaki `:variant`, graph `route_variant`,
   route labels.
