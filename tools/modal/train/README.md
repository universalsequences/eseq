# Trainable modal voices (`eseq.modal.train`)

The feed-forward, gradient-trainable cousin of `eseq.modal` (tools/modal/README.md).
A voice is a list of **parts** (decaying modes, strike sines, noise bands, a mode
template, a second stroke, chain stages). A 4-number latent is decoded per hit into
the parts' controls, and `DGenLisp train-dataset` fits it to a set of samples. Every
part is a sum of decaying oscillators or fixed-filter noise with per-hit gains, and
nothing feeds back, so the GPU fits every sample and frame in parallel.

Design: docs/trainable-modal-spec.md. Code generation is Lisp
(`content/ui/modal/train.lisp`); the runner here is Python (dataset, seeding,
stages, scoring, shipping) and reads what the Lisp writes.

## One file per experiment

```lisp
(import eseq.modal.train :as t)

(def snare
  (t/voice "my-snare" :layout 'snare
    :parts (list (t/modes 'head :count 16 :glide true :tail true :rise :shared :bus :body)
                 (t/strikes 'stick)
                 (t/shaper 'body) (t/clipper 'clip) (t/record-lp 'rec) (t/timing 'time)
                 (t/bands 'wires) (t/hiss 'hiss :on 'wires)
                 (t/crackle 'crackle) (t/events 'ev :on 'wires))))

(t/emit-experiment
  (t/experiment "my-snare-v1" :voice snare
    :data (t/data :packs '("training set 1"))
    :seed (t/seed :analyze :drum)
    :train (t/recipe (t/refine :steps 400 :lr 0.02 :freeze :roles)
                     (t/refine :steps 200 :lr 0.005 :wave 0.3 :freeze '())
                     (t/pick :band 0.3 :mr 1)
                     (t/pretrain :steps 3000)
                     (t/path 0.02 :steps 4000))
    :ship (t/ship "My Snare"))
  ".local/train/my-snare-v1")
```

```sh
target/release/eseq lisp run my-snare.lisp           # writes the experiment folder
$PY tools/modal/train/run.py .local/train/my-snare-v1   # data, seed, train, eval, ship
```

`$PY` needs numpy and scipy (the research venv: `.local/research/latent-from-samples/.venv`).
`run.py` caches every stage in the experiment folder; delete a stage's output to redo it.
`--until data|seed|superset|train|eval` stops early.

The folder holds `voice.lisp` (the training voice), `morph.lisp` (the Morph
instrument's voice, generated from the same parts), `layout.json` (the decoder's
segments with their part, field, role and range; the parts; the lineage) and
`experiment.json`, then `data/`, `rows/`, `runs/`, `report.md` and the staged instrument.

## Parts

| kind | what | notes |
|---|---|---|
| `modes` | decaying sines | `:follow-note`, `:glide` (one shared tension glide), `:tail`, `:rise :shared/:instant`, `:strokes`, `:bus :body/:drum`, `:knob` |
| `strikes` | short gliding sines (the stick) | body bus |
| `shaper` | odd + even body saturation | reads the body bus, DC removed in closed form |
| `clipper` | asymmetric recording clipper | over the drum bus |
| `record-lp` | record low-pass as per-component gains | defines `rlg` / `rlg-hit` / `rlg-at` |
| `timing` | onset delay, end, fade, gain | required |
| `bands` | 16 fixed noise bands, two-stage envelope | snare wires; other parts reuse its `bands` |
| `hiss` / `crackle` / `events` | record noise, crackle, two noise events | `:on` a bank |
| `template` | a fixed mode table warped by size / level / tilt / decay | `:keep N` strongest; `:second :absolute` |
| `wash` | a bank's bands swelling after the stroke | `:second :absolute` |
| `restrike` | a second stroke (time, level) | parts with `:strokes true` follow it |

## Variants

```lisp
(t/variant parent-voice "v4"
  :swap (list (t/template 'ride ... :keep 16 :second :absolute))
  :add  (list (t/modes 'tamb :count 6 :follow-note false :strokes true))
  :remove '(crackle))
```

With `(t/seed :parent "<parent experiment folder>")` the runner:

- starts every sample the parent knew from the parent's fitted rows (by segment name);
- seeds added parts from a render of those rows (the part's seeder: missing spectral lines
  for free modes, energy for a template, the bank's slow level for a wash, the largest
  high-frequency rise for a second stroke);
- converts a relative second stroke to an absolute one exactly (`lvl2 = lvl x restrike level`);
- **superset check**: with the added parts silent, the parent's samples must render as the
  parent did (`runs/superset/superset.json`; a swap is reported as an expected difference);
- reports against the parent per sample group (`report.md`).

## Checking a voice port

`tools/modal/examples/train-break-snare.lisp` is the hand-written break-snare voice
(.local/research/break-snares voice/snare3.lisp) as a patch. Its generated training
voice renders v2's trained rows identically (37 renders, median difference 0, max
1.3e-7 of peak), and its Morph instrument matches the hand-written one to 1e-7
(26 renders including real morphs).

## Lisp gotchas met building this

- Dict keys must be keywords: `merge` silently drops string keys, and there is no
  `keyword` function. Tables keyed by strings are association lists.
- `merge` takes key value pairs, not a second dict (`merged` here takes a dict).
- No `\u` escapes in strings: quote with `(source "text")`.
- `[:,0:16]` inside a quasiquote reads the comma as unquote: emit `(symbol "[:,0:16]")`.
- `[1 ,n]` reads `n]` as a symbol: write `[1 ,n ]`.
