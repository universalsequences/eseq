#!/usr/bin/env python3
"""Isolated macOS PAF feasibility experiment; not a production synthesizer.

Requires NumPy and the repository audition harness. Explicit compiler and audit
paths keep results attributable to one toolchain. See README.md for limitations.
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
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--compiler', required=True, type=Path)
parser.add_argument('--toolchain-root', required=True, type=Path)
parser.add_argument('--audit-tool', required=True, type=Path)
parser.add_argument('--output-dir', required=True, type=Path)
args = parser.parse_args()
if sys.platform != 'darwin':
    parser.error('The current audition ctypes loader and this measurement script are macOS-only.')
WORK = args.output_dir.resolve()
WORK.mkdir(parents=True, exist_ok=True)
sys.dont_write_bytecode = True
probe_source = Path(__file__).with_name('probe.lisp')
(WORK / 'probe.lisp').write_text(probe_source.read_text())
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument

compiler = str(args.compiler.resolve())
toolchain = str(args.toolchain_root.resolve())
os.environ['DGEN_BINARY_AUDIT_TOOL'] = str(args.audit_tool.resolve())


def load(path, frames=128):
    inst = Instrument(str(path), compiler=compiler, toolchain_root=toolchain, max_frames=frames)
    subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                    str(Path(inst.build_dir) / 'patch.c')], check=True)
    return inst


def model(phase, mod_phase, pitch, center, width, depth):
    k = np.floor(center / pitch)
    q = center / pitch - k
    pm = depth * np.sin(2 * np.pi * mod_phase)
    pulse = np.exp(-width ** 2 * np.sin(np.pi * phase) ** 2)
    return pulse * ((1 - q) * np.cos(2 * np.pi * k * phase + pm)
                    + q * np.cos(2 * np.pi * (k + 1) * phase + pm))


def bench(inst):
    # Python/ctypes overhead is INCLUDED. No claim about full app CPU or polyphony.
    block = inst.max_frames
    ins = [np.zeros(block, dtype=np.float32) for _ in range(inst.n_in)]
    outs = [np.zeros(block, dtype=np.float32) for _ in range(inst.n_out)]
    for name, value in [('pitch', 220), ('gate', 1), ('velocity', 1)]:
        if name in inst.inputs:
            ins[inst.inputs[name]][:] = value
    ptr = ctypes.POINTER(ctypes.c_float)
    ip = (ptr * len(ins))(*[a.ctypes.data_as(ptr) for a in ins])
    op = (ptr * len(outs))(*[a.ctypes.data_as(ptr) for a in outs])
    mem = inst.fresh_memory()
    mp = mem.ctypes.data_as(ctypes.c_void_p)
    context = ctypes.byref(inst.context)
    fn = inst.process_fn
    for _ in range(500):
        fn(ip, op, block, mp, context, None)
    times = []
    for _ in range(7):
        start = time.perf_counter_ns()
        for _ in range(2000):
            fn(ip, op, block, mp, context, None)
        times.append((time.perf_counter_ns() - start) / 2000 / 1000)
    return {'median_us_per_128_frames': float(np.median(times)),
            'batch_min_us': min(times), 'batch_max_us': max(times), 'batch_us': times,
            'one_voice_percent_one_core': float(np.median(times) / (128 / 48000 * 1e6) * 100)}


single = load(WORK / 'probe.lisp')
rows = []
for pitch, center, width, depth, ratio in [
        (100, 1200, 2, 0, 2), (200, 1200, 1, 0, 2), (400, 1200, .5, 0, 2),
        (100, 700, 2, 0, 2), (100, 2500, 2, 0, 2),
        (110, 1234, 2, 0, 2), (500, 8000, 2, 8, 8), (1000, 18000, 12, 0, 2)]:
    y, _ = single.render(.5, pitch=pitch, params={'center': center, 'width_index': width,
                                                'pm_depth': depth, 'pm_ratio': ratio})
    ref = model(y[:, 1].astype(float), y[:, 2].astype(float), pitch, center, width, depth)
    error = float(np.max(np.abs(y[:, 0] - ref)))
    assert np.isfinite(y).all() and error < .0002, (pitch, center, error)
    a = y[4800:, 0]
    spec = np.abs(np.fft.rfft(a * np.hanning(len(a))))
    freqs = np.fft.rfftfreq(len(a), 1 / 48000)
    peak = float(freqs[np.argmax(spec[1:]) + 1])
    if depth == 0 and center in (700, 1200, 2500):
        assert abs(peak - center) <= 2.5, (pitch, center, peak)
    rows.append(dict(pitch_hz=pitch, center_hz=center, width_index=width, pm_depth=depth,
                     pm_ratio=ratio, peak_hz=peak, max_abs_error_from_phase_reference=error))

block_rows = []
reference, _ = single.render(.25, pitch=110, params={'center': 1234})
for block in [64, 256]:
    other = load(WORK / 'probe.lisp', frames=block)
    y, _ = other.render(.25, pitch=110, params={'center': 1234})
    delta = float(np.max(np.abs(y - reference)))
    assert delta < 1e-6, delta
    block_rows.append(dict(block_size=block, max_abs_diff=delta))

# A bounded, deliberately non-production eight-stage PM/formant workload.
# Shared fundamental phase, eight independent amplitude EGs, eight noise BPs.
# Bandwidth index deliberately has no physical-Hz claim; no anti-aliasing here.
common = '''; Research workload only; not a factory instrument or FS1R model.
(def pitch (in 1 @name pitch))
(def gate (in 2 @name gate))
(def trigger (in 3 @name trigger))
(param fm_depth @default 0.2 @min 0 @max 2)
(param breath @default 0.1 @min 0 @max 1)
(def ph (phasor pitch))
(defmacro pair (phase frequency center width pm atk dec rel)
  (def r (/ center (max frequency 1)))
  (def k (floor r))
  (def q (- r k))
  (def pulse (exp (* -1 width width (pow (sin (* pi phase)) 2))))
  (def car (+ (* (- 1 q) (cos (+ (* twopi k phase) pm)))
              (* q (cos (+ (* twopi (+ k 1) phase) pm)))))
  (def env (adsr gate trigger atk dec 0.7 rel))
  (def voiced (* env pulse car))
  (def unvoiced (* env 0.125 (svf (noise) center 8 1)))
  (tuple voiced unvoiced))
'''
lines = [common]
for i in range(8):
    pm = '0' if i == 0 else f'(* fm_depth v{i-1})'
    lines.append(f'(def (v{i} n{i}) (pair ph pitch {500+i*550} {1+i*.2:.2f} {pm} {5+i*2} {100+i*10} {200+i*20}))')
voices = ' '.join(f'v{i}' for i in range(8))
noises = ' '.join(f'n{i}' for i in range(8))
lines.append(f'(out (* 0.125 (+ (+ {voices}) (* breath (+ {noises})))) 1 @name audio)')
(WORK / 'bank.lisp').write_text('\n'.join(lines) + '\n')
bank = load(WORK / 'bank.lisp')
audio, _ = bank.render(1.5, pitch=220, gate_off=.5)
assert np.isfinite(audio).all() and np.max(np.abs(audio)) > .001
bank_check = {'peak': float(np.max(np.abs(audio))),
              'rms': float(np.sqrt(np.mean(audio ** 2))),
              'last_100ms_rms': float(np.sqrt(np.mean(audio[-4800:] ** 2)))}

assert bank_check['last_100ms_rms'] < 1e-5, bank_check

# Alias comparison is FLOAT64 ANALYTIC, not compiled DGen audio.
# Integer-Hz periodic one-second signals permit ideal FFT lowpass/downsample.
def alias_case(pitch, center, width, depth, ratio):
    def generate(sr):
        t = np.arange(sr) / sr
        return model((t * pitch) % 1, (t * pitch * ratio) % 1, pitch, center, width, depth)
    native = generate(48000)
    def filtered(factor):
        x = generate(48000 * factor)
        spec = np.fft.rfft(x)[:24001] / factor
        spec[-1] = 2 * spec[-1].real  # combine +/- output-Nyquist bins
        return np.fft.irfft(spec, n=48000)
    ref8 = filtered(8)
    ref16 = filtered(16)
    rms = lambda x: np.sqrt(np.mean(x*x))
    return dict(pitch_hz=pitch, center_hz=center, width_index=width, pm_depth=depth,
                pm_ratio=ratio,
                native_alias_error_db=float(20*np.log10(max(rms(native-ref16)/rms(ref16), 1e-15))),
                reference_8x_vs_16x_error_db=float(20*np.log10(max(rms(ref8-ref16)/rms(ref16), 1e-15))))

results = dict(
    note='Research only. No FS1R hardware comparison, listening verdict, production synth, or full-app CPU measurement.',
    platform=platform.platform(), cpu=subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip(),
    compiler=compiler, compiler_sha256=single.compiler_sha256,
    measurement_script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    source_sha256={p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [WORK/'probe.lisp', WORK/'bank.lisp']},
    sample_rate=48000,
    repository_head=subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip(),
    dgen_lock=(ROOT / 'content/dgenlisp.lock').read_text(),
    preamble_source_sha256=hashlib.sha256((ROOT / 'crates/sequencer/src/lisp_host/dgen/instrument_compile.rs').read_bytes()).hexdigest(),
    static_cases=rows, block_invariance=block_rows, bank_signal=bank_check,
    benchmarks={'single_paf_with_two_phase_diagnostics': bench(single), 'eight_pairs': bench(bank)},
    analytic_alias_cases=[alias_case(*case) for case in [(100,1200,2,0,2), (1000,18000,12,0,2), (500,8000,2,8,8)]],
    build_dirs=[single.build_dir, bank.build_dir])
(WORK / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps(results, indent=2))
