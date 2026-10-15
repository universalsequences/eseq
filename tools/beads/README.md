# Beads effect tooling

Verification for the `content/effects/Beads` factory effect (hand-written
`dsp.lisp`, not generated).

- `ref/bfxref.cc`: renders the original `beads::Processor` over a raw float
  stereo file. Build with `EURORACK=/path/to/pichenettes/eurorack ref/build.sh`
  (set `OUT` for the binary path; `tools/bfx.py` looks in
  `$BEADS_WORK/bfxref`, default `~/.cache/eseq-beads`).
- `tools/bfx.py`: compiles the effect with the pinned DGenLisp (through
  `tools/audition`) and renders it, and drives the reference.
- `tools/compare.py`: sample-exact comparison in hi-fi (grains, freeze, SEED,
  delay mode).
- `tools/lofi.py`: level and band checks for the other media.
- `tools/extract_ulaw.py`: regenerates `data/ulaw.json`.
