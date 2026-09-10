"""Paths, reference decoding and the audited production instrument compiler."""
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import sys

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SAMPLES = ROOT/'samples-to-analyze/Acoustic Cymbals Vol.1 by Donit'
FACTORY = ROOT/'content/instruments/Physical Models'
NAMES = {'crash': 'PM Crash', 'ride': 'PM Ride', 'hihat': 'PM Hi-Hat'}
sys.path.insert(0, str(ROOT/'tools/audition'))
from audition import Instrument


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_reference(path, sr=48000):
    y, rate = sf.read(path, always_2d=True)
    if not np.isfinite(y).all():
        raise ValueError(f'Nonfinite reference: {path}')
    energy = np.mean(y*y, axis=1)
    active = np.flatnonzero(energy > energy.max()*1e-4)
    if not len(active):
        raise ValueError(f'Silent reference: {path}')
    onset = int(active[0])
    y = y[onset:]
    if rate != sr:
        gcd = np.gcd(rate, sr)
        y = resample_poly(y, sr//gcd, rate//gcd, axis=0)
    return y, onset, rate


def instrument(source, sr=48000, block=128):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT/'crates/sequencer/tools'/target))
    result = Instrument(source, compiler=compiler,
        toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),
        sample_rate=sr, max_frames=block)
    audit = subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                            str(Path(result.build_dir)/'patch.c')], capture_output=True, text=True)
    if audit.returncode:
        raise RuntimeError('Generated-C fusion audit failed:\n'+audit.stdout+audit.stderr)
    return result
