# Trainable modal patches (`eseq.modal.train`)

Status: draft rev 1 (2026-11-01). Epic: see `bd show` for "Trainable modal patches".

## 1. Why

The Latent Snare / Latent Break Snare voices are trained from samples with
`DGenLisp train-dataset`: a 4-number latent is decoded per hit into a few
hundred controls of a **feed-forward** voice (sums of decaying oscillators
and fixed-filter noise bands, gains applied per hit, no feedback), which is
why the GPU can fit every sample and frame in parallel and gradients pass
straight through.

Each experiment so far (break-snares STUDY.md: snare1 -> snare2 metal ->
snare3 ride) was hand work in five places that must agree:

- the voice (`snare3.lisp`): `row-layout`, `decoded` / `at-hit` forms, DSP;
- its Morph twin (`snare3_morph.lisp`), kept in sync by hand;
- seed scripts (`seed_metal.py`, `seed_ride.py`) and seed scaling;
- the run script (freeze lists, stage order, pick rule, lambda);
- the checks: "new part silenced = parent exactly" (superset), per-sample
  deltas vs the parent, CPU per block.

`eseq.modal` already shows the shape that removes this: an instrument written
as objects + connections in eseqlisp, code-generated into DGenLisp by Lisp
(`content/ui/modal.lisp`, quasiquoted forms + `pp-forms`), Rust only for
heavy math natives. This spec is its **trainable cousin**: same authoring
model, every object differentiable and feed-forward, plus the data, the
training recipe and the lineage in the same file.

## 2. Principles

1. **Codegen is Lisp.** The module lives in `content/ui/modal/train.lisp`
   (`eseq.modal.train`) on top of `eseq.gen` (`content/ui/gen.lisp`). No
   new Rust except the one generic change in §7.
2. **Feed-forward only.** Objects may read another object's output
   (a rattle gated by its string's envelope) but never feed back. Nonlinear
   contacts of `eseq.modal` (felt, strike, snare-bed) are out of scope for
   gradient training; §9.
3. **One source, every artefact.** Voice, Morph voice, row layout, knob
   identities, seeder list, freeze groups, run script and checks all come
   from the same object list, so they cannot drift.
4. **Experiments are children.** A variant names its parent; warm start,
   superset check and the comparison report are automatic.

## 3. A patch

```lisp
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(import eseq.modal.train :as t)

(t/voice "break-snare" :latent 4 :condition '(velocity)
  :objects (list
    (t/modes    'head  :count 16 :glide :shared :tail 0 :follow-note true)
    (t/strikes  'stick :count 4 :glide true)
    (t/bands    'wires :count 16 :env :two-stage)
    (t/modes    'metal :count 8 :follow-note false)
    (t/template 'ride  :from (m/modal-data 'pm-ride (modal-read-data "ride_template.json"))
                       :learn '(size level tilt decay))
    (t/bands    'wash  :share 'wires :env :swell))
  :strokes (list (t/hit 'main :excites :all)
                 (t/hit 'second :time (t/learn :lin 0.02 0.8) :scaled-by 'rhythm
                                :excites '(ride wash metal)))
  :chain   (list (t/shaper 'body :on '(head)) (t/clipper) (t/record-lp) (t/gate))
  :record  (list (t/hiss :on 'wires) (t/crackle) (t/events 2))
  :knobs   '(tune decay snappy level rhythm metal record)
  :data    (t/samples :packs '("set") :tags '(snare mf-doom) :crop :first-hit :frames 32768)
  :train   (t/recipe (t/refine :freeze '(freq time) :steps 400 :lr 0.02)
                     (t/refine :wave 0.3 :steps 200 :lr 0.005)
                     (t/pick '(0.3 band) '(1 mr))
                     (t/pretrain :steps 3000)
                     (t/path 0.02 :steps 4000))
  :ship    (t/instrument "Latent Break Snare" :morph true :folder "Research"))
```

## 4. Objects

Each object kind is a Lisp function returning a part record:

| key | what |
|---|---|
| `:segments` | its `row-layout` lines: `(name count scale lo hi)` |
| `:per-hit` | forms built from `decoded` segments, held with `at-hit` |
| `:dsp` | per-sample forms; the part's output symbol |
| `:morph` | how each segment morphs (`ramped` / `falling` / `at-hit` / `decaying`) |
| `:silence` | segment values that make the part exactly silent (superset check) |
| `:freeze` | segment groups by role (`freq`, `time`) for the recipe's freeze lists |
| `:seeder` | the seeder name + args (§6) |
| `:knobs` | which knobs scale which segments, identity at their defaults |
| `:cost` | a CPU estimate per block (modes x strokes, bands), reported per part |

First kinds (all exist in snare3 today): `modes` (free or note-tracked,
optional shared glide / slow tail), `strikes`, `bands` (fixed RBJ bank on
one white noise, envelope `:two-stage` or `:swell`, `:share` another part's
filters), `template` (a fixed mode table warped by a few learned numbers),
`hit` (strokes: time, level, which parts it excites), chain stages, record
stages.

**Templates from modal objects.** `:from` takes any `eseq.modal` object or
mode-data file: circular plates, FE solids, measured tables. The table is
solved at generate time with the natives eseq.modal already has; the voice
only learns the warps listed in `:learn`.

**Treatments (later slice).** For prepared strings, all feed-forward:
`mass` (a bolt: per-partial frequency / decay offsets around the bolt
point, inharmonic), `rattle` (noise bands gated by the string's own
envelope), `mute` (rubber / felt: a damping mask over the partials). With
`:condition '(note velocity)` the decoder can learn which note carries which
treatment.

## 5. Generated artefacts

`(t/voice ...)` returns an `eseq.gen` program. Its generator emits:

- `dsp.lisp`: the instrument voice (decoder + parts + chain + knobs);
- `train/voice.lisp`: the row-batched training voice (`train-dataset`'s
  `--patch`);
- `train/morph.lisp`: the Morph voice from the same parts' `:morph`;
- `train/layout.json`: segments, scales, freeze groups, silence values;
- `train/run.sh`: the recipe as `DGenLisp train-dataset` calls (stages,
  freeze lists, pick rule, path lambda, GPU batch splitting: rows per batch
  from frames x the parts' memory estimate, refine stages split, scored
  together after);
- `train/seed.json`: the seeder plan; `train/checks.json`: the superset
  and report plan;
- `ui.lisp`: the panel from `:knobs` via `eseq.gen.panel`.

## 6. Data, seeders, checks

`:data` names packs / tags in the sample library; `run.sh` exports them
(`tools/sample-packs/export_pack.py`) and crops them (first hit, the
break-snares `make_dataset2.py` rules, including one-shots that already
start on the hit). Seeders and scoring stay Python (scipy) for now:
`run.sh` calls them by the names in `seed.json` (`analyze-drums` +
`drum-rows` for the head and wires, `missing-lines` for free modes,
`hf-rise` for a second stroke, `template-energy` for a template's level).
The patch names them; porting them to Lisp is not a goal.

## 7. The one Rust change

The generator result map (`tools/gen/README.md`) has `:name :dsp :ui
:voice-mode :assets :kind`. Add a generic **`:files`** key: a dict of
relative path -> text, written beside dsp.lisp by
`crates/sequencer/src/lisp_host/eseq/generator.rs`. Paths must stay inside
the instrument folder. Useful to any generator; nothing modal-specific.

## 8. Variants and lineage

```lisp
(t/variant "break-snare-v4" :from "break-snare-v2.lisp"
  :add    (list (t/modes 'tamb :count 6 :follow-note false))
  :swap   (list (t/template 'ride :count 16))
  :change (list (t/hit 'second :level :absolute)))
```

`t/variant` is a Lisp function from the parent program to a child. Comparing
the two object lists gives:

- **warm start**: the parent's fitted rows for unchanged parts, seeders for
  new ones (what v1 / v2 / the combined run did by hand);
- **superset check** (`:add`, `:change` with a neutral setting): with the new
  parts at their `:silence` values the child's step-0 render must equal the
  parent's; a `:swap` has no silent equivalent and gets a "step 0 vs parent"
  score instead, labelled as such;
- **report**: band / MR median and p90, per-sample deltas, per-tag deltas,
  latent CV / sag / hot, CPU per part, vs the parent (the STUDY.md tables);
- **lineage**: each run records its parent; a variant can be scored on the
  parent's dataset or a held-out pack to catch regressions.

## 9. Out of scope (for now)

- Nonlinear contacts with feedback (wire buzz as a contact, felt, strike):
  approximate them feed-forward, or fit a few of their physical numbers
  without gradients (`--cma-forward-batch`) in a later slice.
- Training from inside the app (C-c C-c -> train) needs a process-spawn
  native; wait until the CLI workflow has proven itself.

## 10. Slices

1. **Spec** (this document).
2. **`:files` + module skeleton + snare3 port**: `eseq.modal.train` with the
   snare3 parts; the generated training voice and Morph voice must render
   the same audio as the hand-written `snare3.lisp` / `snare3_morph.lisp`
   for the m2_p0.02 rows (bit-identical or within float rounding).
3. **Recipe + data**: `run.sh` with batch splitting, export + crop, seeder
   calls; reproduce the combined run (eseq-w9yz.6).
4. **Variants**: warm start, superset check, comparison report; first use:
   break-snare v4 (absolute re-strike for B28, 16-mode ride, tambourine).
5. **Templates from modal objects + treatments**: circ-plate / FE templates,
   `mass` / `rattle` / `mute`; first new voice: a prepared piano trained on a
   pack of treated piano samples.
