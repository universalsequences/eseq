#!/usr/bin/env python3
"""Bounded spectral voicing search using the compiled physical model.

This finds a starting voicing, not an identified clarinet or a sample player.
Writes an experiment report only; curated factory defaults are reviewed separately.
"""
import json
from pathlib import Path
import platform
import subprocess
import sys

import numpy as np
from scipy.optimize import differential_evolution

from analyze import ROOT, harmonics
sys.path.insert(0, str(ROOT/'tools/audition'))
from audition import Instrument


def main():
    reference = json.loads((Path(__file__).parent/'reference-analysis.json').read_text())
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    instrument = Instrument(ROOT/'content/instruments/Physical Models/PM Clarinet',
        compiler=str(ROOT/'crates/sequencer/tools'/target),
        toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'))
    subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                    str(Path(instrument.build_dir)/'patch.c')], check=True)
    names = ['reed.pressure', 'reed.stiffness', 'reed.closure', 'reed.curve',
             'bore.damping_hz', 'bore.loss', 'color.body', 'color.body_hz', 'color.body_q', 'color.bell_hz']
    bounds = [(.5, 1.1), (.16, .55), (.5, .82), (.55, 2.5),
              (1800, 11000), (.005, .1), (.5, .9), (650, 1150), (1.5, 5.5), (3500, 14000)]
    target = np.maximum(reference['harmonics_db'], -50)
    weights = np.array([3, 1, 3, 2, 2, 1, 1, .5, 1, .5, .5, .4, .3, .2, .2, .2])
    best = [float('inf'), None]
    count = [0]

    def objective(values):
        params = dict(zip(names, values))
        y, state = instrument.render(2, pitch=reference['fundamental_hz'], params=params)
        count[0] += 1
        if not np.all(np.isfinite(state)) or not np.all(np.isfinite(y)):
            raise RuntimeError('Non-finite physical model during voicing')
        level = float(np.sqrt(np.mean(y[24000:]**2)))
        if level < .003 or max(abs(y)) > 2:
            return 10000
        measured = harmonics(y, 48000, reference['fundamental_hz'], .5, 1.8)
        loss = float(np.average((np.maximum(measured, -50)-target)**2, weights=weights))
        if loss < best[0]:
            best[:] = [loss, {'params': params, 'harmonics_db': measured.tolist(), 'rms': level}]
        return loss

    seed = [.75, .3, .7, 1, 4500, .035, .88, 887, 4.2, 11000]
    baseline = objective(seed)
    result = differential_evolution(objective, bounds, seed=17, x0=seed,
        maxiter=24, popsize=6, polish=False, workers=1,
        callback=lambda x, convergence: print(f'{count[0]} renders / spectral MSE {best[0]:.3f}', flush=True))
    report = {'reference_sha256': reference['sha256'], 'compiler_sha256': instrument.compiler_sha256,
              'evaluations': count[0], 'initial_spectral_mse': baseline,
              'best_spectral_mse': best[0], 'best': best[1], 'optimizer_message': str(result.message)}
    output = Path(__file__).parent/'output'
    output.mkdir(exist_ok=True)
    (output/'fit.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
