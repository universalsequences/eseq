#!/usr/bin/env python3
"""Validate the compiled finite-series kernel against an explicit harmonic sum.

Uses the published compiler, never a local Swift build. Artifacts go outside
factory content until the DSP and integration gates pass.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument


def reference(phase, f0, center, bandwidth, skirt, pm, sr):
    h = np.arange(1, int(sr * .5 / f0) + 1, dtype=float)
    distance = 2 * np.abs(h * f0 - center) / bandwidth
    amps = (skirt * np.exp(-np.log(2) * distance)
            + (1 - skirt) * .5 * (np.exp(-np.log(4) * distance)
                                  + np.exp(-np.log(4 / 3) * distance)))
    amps *= .5 * (1 - np.cos(np.pi * np.clip((sr * .5 / f0 - h) / 2, 0, 1)))
    amps /= max(1, amps.sum())
    out = np.zeros_like(phase, dtype=float)
    for harmonic, amp in zip(h, amps):
        out += amp * np.cos(harmonic * phase + pm)
    return out


def benchmark(inst):
    count = inst.max_frames
    ptr = ctypes.POINTER(ctypes.c_float)
    inputs = [np.zeros(count, np.float32) for _ in range(inst.n_in)]
    inputs[inst.inputs['pitch']][:] = 220
    for name in ('gate', 'velocity'):
        if name in inst.inputs:
            inputs[inst.inputs[name]][:] = 1
    outputs = [np.zeros(count, np.float32) for _ in range(inst.n_out)]
    ip = (ptr * len(inputs))(*[x.ctypes.data_as(ptr) for x in inputs])
    op = (ptr * len(outputs))(*[x.ctypes.data_as(ptr) for x in outputs])
    memory = inst.fresh_memory()
    mp = memory.ctypes.data_as(ctypes.c_void_p)
    ctx = ctypes.byref(inst.context)
    if 'trigger' in inst.inputs:
        inputs[inst.inputs['trigger']][0] = 1
        inst.process_fn(ip, op, count, mp, ctx, None)
        inputs[inst.inputs['trigger']][0] = 0
    for _ in range(100):
        inst.process_fn(ip, op, count, mp, ctx, None)
    times = []
    for _ in range(9):
        start = time.perf_counter_ns()
        for _ in range(500):
            inst.process_fn(ip, op, count, mp, ctx, None)
        times.append((time.perf_counter_ns() - start) / 500 / 1000)
    return dict(batch_mean_us=times, median_us=float(np.median(times)))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--audit-tool', required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    os.environ['DGEN_BINARY_AUDIT_TOOL'] = args.audit_tool
    kernel = Path(__file__).with_name('kernel.lisp').read_text()
    source = kernel + '''
(def pitch (in 1 @name pitch))
(param center @default 1200 @min 10 @max 20000)
(param bandwidth @default 300 @min 20 @max 8000)
(param skirt @default 0.5 @min 0 @max 1)
(param pm @default 0 @min -16 @max 16)
(def ph (* twopi (phasor (clip pitch 10 (* 0.49 samplerate)))))
(out (ff-formant ph pitch center bandwidth skirt pm) 1 @name audio)
(out ph 2 @name phase)
'''
    path = args.output / 'kernel-probe.lisp'
    path.write_text(source)
    compiler = ROOT / 'crates/sequencer/tools/DGenLisp-macos-arm64'
    options = dict(compiler=str(compiler), toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'))
    rows = []
    for sr in (44100, 48000, 96000):
        inst = Instrument(str(path), sample_rate=sr, max_frames=128, **options)
        subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                        str(Path(inst.build_dir) / 'patch.c')], check=True)
        for f0, fc, bw in [(55, 1234, 20), (110, 1200, 300), (440, 5000, 1200),
                           (10, 80, 8000), (1000, 16000, 8000), (10000, 80, 20),
                           (220, 1320 - .001, 400), (220, 1320 + .001, 400)]:
            for skirt in (0, .5, 1):
                params = dict(center=fc, bandwidth=bw, skirt=skirt, pm=.3)
                y, _ = inst.render(.03, pitch=f0, params=params)
                ref = reference(y[:, 1].astype(float), f0, fc, bw, skirt, .3, sr)
                error = float(np.max(np.abs(y[:, 0] - ref)))
                relative = float(np.sqrt(np.mean((y[:, 0] - ref)**2)) / max(1e-12, np.sqrt(np.mean(ref**2))))
                rows.append(dict(sr=sr, f0=f0, center=fc, bandwidth=bw, skirt=skirt,
                                 peak_error=error, relative_error_db=20*np.log10(max(relative, 1e-15))))
                assert np.isfinite(y).all(), rows[-1]
                assert error < 2e-4, rows[-1]
    results = dict(platform=platform.platform(),
                   compiler_sha256=hashlib.sha256(compiler.read_bytes()).hexdigest(),
                   source_sha256=hashlib.sha256(source.encode()).hexdigest(),
                   cases=rows, benchmark_96k=benchmark(inst))
    (args.output / 'results.json').write_text(json.dumps(results, indent=2)+'\n')
    print(json.dumps(dict(max_peak_error=max(r['peak_error'] for r in rows),
                          benchmark_96k=results['benchmark_96k']), indent=2))


if __name__ == '__main__':
    main()
