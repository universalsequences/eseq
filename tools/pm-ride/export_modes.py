#!/usr/bin/env python3
"""Write PM Ride Kit's identified model as an eseq-modes v1 file (build.modes_file).

The same coefficients build.py bakes into the factory dsp.lisp, as data an
eseq.modal patch loads with (modal-read-data "ride.modes.json") and plays
through m/modal-data and m/strokes (tools/modal/examples/ride-kit.lisp):

  freqs, losses     the 688 plate modes (Hz, decay rate 1/s)
  points, shapes    point 0 is the microphone, every mode's shape 1 (each
                    stroke's residue is already what the mic heard); point
                    k + 1 is where stroke k struck: the shape whose impulse
                    reproduces the stroke's residue (see stroke_shapes), so
                    a physical stick, stop or rivet can act there
  measured          true: the losses are measured, so eseq.modal adds no air
                    loss of its own
  strokes           one per key from C: name, re/im (the complex residue a
                    hit injects: im is what is heard at once, re a quarter
                    cycle later), click (rms, decay_s, hz)
  contact_s         the stick contact time the residues were measured with
  wash              the third-octave band-noise layer standing in for the
                    weak modes not run individually (band hz, buildup rate,
                    fast/slow decay rates, per-stroke fast/slow powers)

Only fitted coefficients: no PCM, recorded phase or spectral frames.
"""
import argparse
import json
from pathlib import Path

import build


def main():
    """Write the file; build.py also writes it, and --check verifies it."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('out', nargs='?', type=Path, default=build.MODES_OUT)
    args = parser.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(build.modes_text(json.loads((build.HERE/'analysis.json').read_bytes())))
    print('Wrote', args.out)


if __name__ == '__main__':
    main()
