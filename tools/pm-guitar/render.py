#!/usr/bin/env python3
"""Compile PM Nylon Guitar through the production compiler and render test notes (local only)."""
import os
import platform
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT/'tools/audition'))
os.environ.setdefault('AUDITION_CACHE', str(ROOT/'.local/pm-guitar/cache'))
from audition import Instrument   # noqa: E402
from build import DEST            # noqa: E402

OUT = ROOT/'.local/pm-guitar'


def instrument(sr=48000, block=128, voices=1):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64', ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    return Instrument(str(DEST), compiler=os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT/'crates/sequencer/tools'/target)),
                      toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'), sample_rate=sr, max_frames=block, voices=voices)


def hz(note):
    return 440*2**((note - 69)/12)


def main():
    inst = instrument()
    out = []
    # a chromatic-ish walk through the range at three velocities, then a chord built from single voices
    for note in [40, 45, 49, 52, 56, 59, 61, 64, 65, 67, 69, 72, 76, 79, 84]:
        for vel in (0.35, 0.7, 1.0):
            y, _ = inst.render(seconds=1.1, pitch=hz(note), vel=vel, gate_off=0.8)
            assert np.isfinite(y).all()
            out.append(y)
    chord = sum(inst.render(seconds=3.0, pitch=hz(n), vel=.75, gate_off=2.5)[0] for n in (49, 56, 61, 65, 68))
    out.append(chord)
    y = np.concatenate(out)
    # the first recorded pluck's key (C#2) with and without the fret, then the
    # low string at a few pitches/velocities, then buzz spread to all strings
    demo = []
    gap = np.zeros((24000, 2), np.float32)
    for params in ({}, {'fret.buzz': 0.0}):
        demo += [inst.render(seconds=1.6, pitch=hz(37), vel=0.6, params={'pluck.humanize': 0.} | params)[0], gap]
    for note, vel in ((37, .4), (37, .9), (39, .7), (36, .8), (40, 1.)):
        demo += [inst.render(seconds=1.2, pitch=hz(note), vel=vel)[0], gap]
    for note in (44, 53, 55, 60):
        demo += [inst.render(seconds=1.0, pitch=hz(note), vel=.8, params={'fret.spread': 1.})[0], gap]
    sf.write(OUT/'buzz-demo.wav', np.concatenate(demo), 48000)
    print('wrote', OUT/'buzz-demo.wav')
    print('peak', float(np.abs(y).max()))
    OUT.mkdir(parents=True, exist_ok=True)
    sf.write(OUT/'scale.wav', y, 48000)
    print('wrote', OUT/'scale.wav')


if __name__ == '__main__':
    main()
