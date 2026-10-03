# Harmony declaration: one source says "this is the chord", voices relate to it

Status: design rev 3, 2026-10-01 (rev 3: the route is `-> chords`). Rev 2: the declaration is a jaki route
(`-> chords`), not a separate sequencer. Not built.
Epic: `bd list --label harmony-decl`.

## 1. Problem

Every jaki row picks absolute notes on its own (`(note (seq :hit 0 4 7))`),
and `harmony` / `scale` can only correct them afterwards. Nothing in the
system knows what chord the music is in, so nothing can relate to it, lean
against it, or move it on. The result is noodling.

The goal is the Autechre / Stafford Beer move: a few simple, mostly
monophonic entities, coupled through a shared harmonic state, producing
progressions that can range from strict to surreal.

## 2. The principle: declaration

Exactly one kind of thing **declares** the harmony: "from bar 5, the chord
is Fm7". Everything else **relates** to the declaration: "play its 3rd",
"take the nearest free chord tone", "hold to it at strictness 0.5".

- The declaration is the single source of truth (Beer's System 3: what the
  whole is doing right now).
- Voices are System 1: monophonic lines that read the field and choose
  relative to it.
- Coordination between voices (not doubling, smooth motion) is System 2.
- Deliberate rule-breaking (substitutions, deceptive moves) is System 4,
  and lives in the declaration's own rules.
- Strictness (how tightly voices obey) is System 5, the style dial.

Emergence (the field drifting from what voices play) is allowed later
(§8), but it decorates a declared spine; it does not replace it.

## 3. The field

What a declaration publishes and voices read:

| Part | Meaning |
|---|---|
| root | pitch class 0–11 (relative to transpose 0, as jaki notes are) |
| quality | `maj min 7 maj7 m7 dim dim7 m7b5 aug sus2 sus4 6 m6 9 m9 add9 7b9 7#9 …` |
| tones | the chord's pitch classes in order: root, 3rd, 5th, 7th, 9th… |
| key | a scale (`:minor`, `:dorian` …) and tonic, for "in key" tests |
| name | display text, `Fm7` |

One named field per declaration, default name `"harmony"`. A project may
have several (a verse and a bass-line field), selected by name.

**Transport:** the existing typed field bus. A declaration publishes with
the equivalent of `(suggest :harmony (pitch-field tones :root r))`, so the
existing harmony cards on step tracks and neurons (`hear`) follow it with no
new plumbing. jaki generators read it through a per-chunk snapshot native,
`(gen-field "harmony")`, like `gen-track-harmony` (one small Rust read; the
generator VM cannot `hear`).

## 4. The declaring row (`-> chords`)

No separate sequencer: a jaki row declares, the way `-> (mute T)` makes a
control route. `chords` sits where a track number goes: `-> 0` plays track
0, `-> chords` plays the chords. Its hits drive the field instead of a track, so the
declaration, the voices and the rule-breaking live in one instance, one tab.

```lisp
(jak "song" :16
  (fig (-) (slow 8))                                   ; one hit = one chord
  -> chords (note (seq :fig 0 5 10 3)) (chord :m7)  ; Cm7 Fm7 Bb7 Ebm7
  -> 0 (note (deg 1)) (note+ -24)
  -> 1 (voice)
  -> 2 (voice))
```

- **Which chord**: the row's note words give the root (seqs, cycle lists,
  rules all apply), `(chord QUALITY)` the quality (default `maj`). A
  declaring hit's final note is the root.
- **When it changes**: the row's own rhythm (figures, `slow`, `every`); a
  declared chord holds until the row's next hit.
- **Rule-breaking**: ordinary rules on that row, e.g.
  `(rule any (coin :p 0.3) (note+ 6))` is a tritone substitution. Dedicated
  words (`sub`, `borrow`, §7) are sugar over this.
- **Name**: `-> chords` writes the default field; `-> (chords "bass")`
  a named one. In the kind, the route dropdown gets a **Chords** entry.
- **Same-beat reads**: voices in the same instance read the declaration for
  the current position by evaluating the declaring route's cycle (pure, seek
  exact): no bus, no latency. Several patterns of one instance share it.
- **Optional audition**: `-> (chords :play 3)` also sounds the voicing on
  track 3.

Outside listeners (step tracks, neurons, other jaki instances) get the
field through the typed bus (§3, slice 6); that is the only place latency
can appear.

## 5. Voices: relating to the field (jaki words)

```lisp
(note (deg 3))            ; the field's 3rd (chord tone 2), nearest octave
(note (deg 5 :of 0))      ; a 5th above what row 0 is playing now
(chord :m7 :inv 1)        ; a bundle on this hit: quality word, root from note/deg
(voice)                   ; nearest chord tone no other voice has taken
(voice :pref (3 7))       ; prefer the 3rd and 7th (guide tones)
(harmony :amount 0.5)     ; existing, now reading the field by default
```

- `deg` counts chord tones (1 root, 3 third, 5 fifth, 7 seventh, 9 ninth),
  falling back to scale degrees of the key for tones the chord lacks.
- `chord` emits several notes on one hit (seq-emit chord); voicing words
  `:inv n`, `:drop2`, `:spread`.
- `voice` keeps per-row memory (its last note) and moves to the nearest
  chord tone when the field changes: voice leading for free. Rows of one
  instance **claim** tones in row order, so four `(voice)` rows form a
  four-note chord without doubling.

## 6. Example

Two voice lines in one instance: a slow declaring line and the band.

```lisp
(jak "band" :16
  ((fig (-) (slow 8))
   -> chords (note (seq :fig 0 5 10 3)) (chord :m7))   ; Cm7 Fm7 Bb7 Ebm7
  (. . - .
   -> 0 (note (deg 1)) (note+ -24)                        ; bass: roots
   -> 1 (voice)                                           ; three voices form
   -> 2 (voice) (rule any (coin :p 0.1) (note+ 1))        ;   the chord, one
   -> 3 (voice)))                                         ;   leaning chromatic
```

## 7. The rule-breaker (on the declaration)

Rules on the declaring row transform the declared chord, as they transform
any hit. Plain words already do a lot (`(note+ 6)` = tritone sub); these are
sugar:

| Word | Effect |
|---|---|
| `(sub :tritone)` | dominant → its tritone substitute |
| `(sub :relative)` | major ↔ relative minor |
| `(borrow :minor)` | take the chord from the parallel mode |
| `(move :fifths n)` | root moves by n fifths |
| `(quality :m7)` | recolor the quality |
| `(skip)` / `(repeat)` | deceptive: replace with the next / hold the previous |

Same rule surface as jaki rules (trigger → stages → actions, `if`, coins
hashed by position), so a progression varies but replays exactly.

## 8. Emergence (later)

- **Home and gravity:** with no declared chord, the field is the key's
  tonic; movers push it away, gravity pulls it back over time.
- **Feedback:** the field may be blended with what the voices actually
  sound (`harmonic-analysis` exists), closing the loop between declaration
  and voices.

## 9. Slices

1. **Field model + chord vocabulary** (Lisp): quality → tones, degrees,
   names.
2. **Declaring route**: `-> chords` / `-> (chords NAME)`, `(chord QUALITY)` on it,
   same-beat reads inside the instance, Chords entry in the kind's route
   dropdown, current chord shown on the row; jaki `(harmony)` defaults to the
   declared field.
3. **Relative voices**: `(deg n [:of row])`, `(chord QUALITY …)`.
4. **Voice leading + claiming**: `(voice …)`.
5. **Rule-breaker words on the declaring row**: `sub borrow move quality
   skip repeat`.
6. **Cross-system**: publish declared fields on the typed bus (`suggest
   :harmony` equivalent) with a `gen-field` read for other instances; step /
   neuron harmony cards select a declared field as their source.
7. **Emergence**: home, gravity, feedback (design first).

## 10. Open questions

- Field timing across instances (slice 6): the per-chunk timeline shape of
  `gen-track-harmony` is the model for same-beat reads from another
  instance's declaration.
- Claiming across instances (rows of two jaki instances both `(voice)`):
  per field, not per instance?

## 11. Built (2026-10-01, slices .1-.4)

- `content/packages/alez.jaki/src/chords.lisp` (module `alez.jaki.chords`):
  26 qualities (`:maj :min :dom7 :maj7 :min7 :hdim7 :dim :dim7 :aug :sus2
  :sus4 :dom7sus4 :maj6 :min6 :minmaj7 :add9 :minadd9 :dom9 :maj9 :min9
  :dom11 :min11 :dom13 :dom7b9 :dom7s9 :power`), each with tones and a scale
  whose even steps are the tones, so `(deg n)` reads chord tones on odd n
  and the scale elsewhere. The field is two generator state cells per name
  (`jf:<name>:root`, `jf:<name>:q`).
- `-> chords` / `-> (chords "name" :play track)`: a note route whose hits
  call `field-set!` instead of `seq-emit` (and stamp a `chord` mark for the
  panel); chord routes play first in every tick (`play-routes`), so voices
  see a new chord on the same tick. The declared chord holds until the next
  declaring hit; a seek keeps the last declared chord until then.
- `(deg n)`: a value form, deferred to emit (`needs-ev?`), resolved by
  `deg-value` against the field (C major before any declaration).
- `(chord Q [:inv n])` on a track row: one `seq-emit` per chord note;
  `(chord)` / `(chord :declared)` plays the declared chord transposed by the
  hit's note. A list of qualities cycles per cycle.
- `(voice)`: `voice-pick` per hit, claims reset each tick (`jv:tick`,
  `jv:mask`), last note per route key (`jv:<rkey>:last`). `:pref` not built.
- Kind: route -2 = **Chords** (last route-dropdown entry); the row shows the
  chord being declared (subtree on `generator-mark-<id>-chord`); row schema
  offers `(chord Q)` (with `:declared`), `voice`, and `(deg n)` inside note /
  note+ values. Fixture: `ui/capture-fixtures/jaki-chords-row.lisp`.
- Not yet: `(deg n :of row)`, `:pref`, named fields in the kind (always
  `"chords"`), the stopped panel showing the authored chord.

## 12. Rev 4: chord words in the Chords lane (2026-10-01)

The Chords lane names chords as single words; they are values, so every jaki
value rule applies:

```lisp
-> chords (chord (Am7 Dm7 G7 Cmaj7))               ; one per cycle
-> chords (chord (Am7 (seq :fig Emaj Fmaj)))       ; nested clocks
-> chords (key A :minor) (chord (i iv V7 (seq :fig bVI bVII)))
-> chords (chord C) (on accent (chord E7)) (every 4 (chord V7/V)) (note+ 5)
```

- **Symbols**: root `A`–`G` with `#` / `b`, then a suffix (`m 7 maj7 m7 m7b5
  ø dim ° dim7 aug + sus2 sus4 7sus4 6 m6 mmaj7 add9 madd9 9 maj9 m9 11 m11
  13 7b9 7#9 5`; `Δ`, `M7`, `-7` also read).
- **Numerals**: chromatic convention, counted from the key's tonic on the
  major scale; case gives the triad, suffixes as above (lowercase reads
  minor-first: `iv7` = m7); accidentals prefix (`bVI`); secondaries with
  `/` (`V7/iv`). The key's mode does not change roots.
- At route build `chord-value` encodes words to numbers
  (`alez.jaki.chords/chord-code`: 10000+root·64+q absolute, 20000+interval·64+q
  relative); `emit-notes` decodes against `(key …)` and adds the hit's note,
  so `note` / `note+` transpose the progression. `(chord …)` inside `on` /
  `every` is a per-hit post op (`:chordv`), taking precedence over the row's.
- On a track row a chord word plays that chord: `-> 1 (chord Am7)`.
- The kind gives Chords rows their own slot schema (`chords-row-schema`):
  chord words, `key`, `seq`, `on`, `every`, `note+`; a fresh Chords row
  starts as `(key C :major) (chord (I IV V7 I))`.
