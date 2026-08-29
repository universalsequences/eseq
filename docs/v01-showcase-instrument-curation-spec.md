# v0.1 Showcase Instrument Curation Spec

Status: brainstorm / working notes (2026-08-28). Captures the discussion for
eseq-2k9p.2 (curate 5–10 showcase instruments + shipped-content manifest).
Not yet a decision record.

## Thesis: specialists, not master synths

Do not build a handful of do-everything "master synths" for v0.1. Ship
**bespoke, weird, purpose-built instruments** instead — the DGen approach:

1. **Very usable** — a narrow range of tasks, done extremely well, with strong
   default presets.
2. **Interpretable** — opening the patch editor shows a cleanly organized,
   documented patch. Forking (already exists) must feel like a natural way to
   make them your own.
3. **Performant as hell** — narrow purpose is what buys the CPU budget.
4. **Collectively broad** — the *set* covers a decent task array (drums,
   808/909, poly, bass, FM, signature), not any single instrument.

The patch source IS the product. A 120-line bespoke cymbal demonstrates what
eseq is; a giant generalist synth hides it.

### Package system defuses the stakes

With the package system, packs of instruments/samples/effects/sequencers can
be published by anyone, including the author — so there is no need for any of
this to be a stressed "day 1 factory instrument" deliverable. It would be
great if day 1 works well without packages, but packs can follow quickly
after release. This makes the curation bar "coherent and delightful," not
"complete."

## Candidate inventory (from discussion)

### Curate / keep as-is (low effort, high identity)

- **md-cym** (`content/instruments/drums/md-cymbal`) — the 808-cymbal-sounding
  mode is the only good one (can even do hi-hat). Alec uses default mode
  exclusively. The other modes reduce performability and suck. **Action: strip
  to the one good mode** — less param surface = more performant + more
  legible. Deleting bad modes is curation, not loss. Same treatment for any
  md-* siblings with one great mode.
- **membrane-snare-rim** (`content/instruments/drums/membrane-snare-rim`) —
  the flagship of the physical-modeling story. Finite-difference 2D membrane
  (6x6 FDTD heads, 6 conv2d/sample — heavy). **Ship the current FDTD version
  in v0.1**; the modal recreation (eseq-80dh) lands later as a v0.1.x pack
  drop — a nice story ("the author forks his own instrument").
- **membrane-tabla** — same story: great, modal version would be a tight,
  more performant follow-up (post-v0.1).

### Widen-the-range candidates

- **synthid-808 / synthid-909** — dgen backprop-recreated literal 808/909
  samples. Doubly valuable: bread-and-butter usability AND marketing story
  ("backprop recreated a 909"). Controls are ultra-limited because training
  pinned each to a single target sample. **Action: widen the param range**
  (tune / decay / drift / noise injection) without retraining — get ~70% of
  the weirdness for ~10% of the effort.

### Cleanup project

- **fullrevfm** — the "eseq-y" flagship. Defines a tuned reverb per voice,
  giving a super Autechre-y feel; near-vocal sounds especially when treated.
  Something people can't get elsewhere. **Action: treat as the primary
  curation effort** — clean patch organization, doc comments in the patch
  source, small set of strong presets (vocal-ish ones especially).

### Gap fillers (poly / bass / FM)

- **Poly**: `drift` or `analog-bread-and-butter` — pick ONE, curate + polish.
  Not a new build.
- **Bass**: existing candidates in `content/instruments/bass/`
  (tb303-basic, morph1, synthid-hoodie-bass, korg1, bad-subbass1). Curation +
  polish job, not a new build. tb303-basic + one B&B variant could cover bass
  for v0.1.
- **FM**: the ONE real greenfield build. Build from scratch (existing
  digitone/operator synths are too complex to curate into something clean).
  Slots directly into **eseq-i2pw** (factory macro library spec): the spec
  already names "greenfield FM" as one of the three rebuild exemplars
  (wavetable + stripped analog-b&b + greenfield FM). The FM synth and the
  macro library are the same work item, sequenced bottom-up: build FM,
  extract macros on second use (Cmd+E encapsulate), and the other instruments
  inherit the vocabulary.

## Proposed v0.1 slate (7 slots)

| Slot | Instrument | Work type |
|---|---|---|
| Drums — 808 cym | md-cym (single mode) | strip modes |
| Drums — 808/909 | synthid-808 + synthid-909 | widen params |
| Drums — physical model | membrane-snare-rim | polish (modal → later pack) |
| Signature | fullrevfm | cleanup + presets |
| Poly | drift OR analog-b&b (pick one) | curation |
| Bass | tb303-basic OR synthid-hoodie-bass | curation |
| FM | greenfield (with eseq-i2pw macros) | build |

Roughly 1.5 real builds; the rest is curation. Fits v0.1.

## Risks / things that must be true

1. **"Interpretable" must be enforced, not hand-tidied.** The clean-patch goal
   only survives if the factory macro vocabulary (eseq-i2pw) is the mechanism.
   Hand-tidying each patch won't stay clean as instruments evolve. So
   **eseq-i2pw is the true first bead**, even before finalizing the
   instrument list.
2. **Manifest needs the mechanical closure rule.** The shipped manifest
   (feeding eseq-4tr.2) must resolve the transitive macro+asset closure
   mechanically — instruments now share factory macros, so a naive
   copy-the-tree manifest breaks.
3. **(mod param) cannot appear inside macro bodies** (eseq-i2pw constraint):
   macros take resolved signals. Factory macro names/arities become public
   API at v0.1 — namespace them from day one, keep ≤15 macros.
4. Every factory macro ships a hand-authored layout sidecar so the patch
   editor view stays legible.

## Open questions

- When stripping md-cym's bad modes: do they die entirely, or fork off as
  separate single-purpose instruments (md-cym vs md-cym-alt)? Single-purpose
  per instrument is more on-thesis, but multiplying instruments has curation
  cost. Default lean: die entirely; fork later if a mode is missed.
- Pick one: drift vs analog-b&b for the poly slot.
- Pick one: tb303-basic vs synthid-hoodie-bass for the bass slot.
- Whether membrane-tabla (and kalimrim, membrane-hat, etc.) ride along in the
  manifest if cheap, or wait for pack drops.

## Related beads

- eseq-2k9p.2 — this curation task + shipped-content manifest (blocks 2k9p.3)
- eseq-2k9p.3 — first-run demo project (consumes the slate)
- eseq-i2pw — factory macro library (spec: docs/factory-macro-library-spec.md)
- eseq-80dh — modal recreation of membrane-snare-rim (post-v0.1 pack drop)
- eseq-26u — patcher asset references (@asset binding model for user-swappable tables)
- eseq-4tr.2 — dist/macos/build.sh (consumes the manifest)
