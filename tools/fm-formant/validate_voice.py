#!/usr/bin/env python3
"""Compile and exercise the complete candidate, preserving raw measurements."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import wave

import numpy as np

sys.dont_write_bytecode = True
from build_instrument import build, default_motion
from presets import write_bank
from validate_kernel import Instrument, ROOT, benchmark


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--audit-tool', required=True)
    args = parser.parse_args()
    os.environ['DGEN_BINARY_AUDIT_TOOL'] = args.audit_tool
    params = build(args.output, default_motion())
    bank = write_bank(args.output, params)
    kwargs = dict(compiler=str(ROOT/'crates/sequencer/tools/DGenLisp-macos-arm64'),
                  toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'))
    inst = Instrument(str(args.output), max_frames=128, **kwargs)
    subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                    str(Path(inst.build_dir)/'patch.c')], check=True)
    assert {name for name in inst.params if not name.startswith('__mod__')} == set(params)
    results = dict(compiler_sha256=inst.compiler_sha256, build_dir=inst.build_dir,
                   source_sha256=hashlib.sha256((args.output/'dsp.lisp').read_bytes()).hexdigest(),
                   motion_sha256=hashlib.sha256((args.output/'motion-tensor.json').read_bytes()).hexdigest(),
                   presets=[], block_checks=[])
    for preset in bank['presets']:
        p = preset['params']
        for pitch in (110, 440, 1760):
            y, _ = inst.render(2.5, pitch=pitch, params=p, gate_off=.5)
            row = dict(name=preset['name'], pitch=pitch, peak=float(np.max(np.abs(y))),
                       rms=float(np.sqrt(np.mean(y*y))),
                       tail_rms=float(np.sqrt(np.mean(y[-2400:]**2))))
            results['presets'].append(row)
            assert np.isfinite(y).all() and row['peak'] < 4 and row['rms'] > 1e-5, row
            assert row['tail_rms'] < 1e-5, row
        print(f'Validated {preset["name"]}', flush=True)
    original, _ = inst.render(.2, pitch=123, params={'breath':0})
    for block in (32,64,256):
        other = Instrument(str(args.output), max_frames=block, **kwargs)
        y, _ = other.render(.2, pitch=123, params={'breath':0})
        error = float(np.max(np.abs(original-y)))
        results['block_checks'].append(dict(block=block, peak_error=error))
        assert error < 1e-5, results['block_checks'][-1]
    # Same graph and initial seed, identical output including the noise bands.
    one, _ = inst.render(.2, pitch=220)
    two, _ = inst.render(.2, pitch=220)
    assert np.array_equal(one,two)
    # Benchmark held notes with all eight generators/envelopes and motion active.
    results['single_voice_microbenchmark'] = benchmark(inst)
    # A dry chord demonstration, rendered sequentially with distinct documented
    # seeds. This is a WAV, not a simultaneous-host polyphony benchmark.
    audio = np.zeros((48000*5,2), np.float32)
    for i,pitch in enumerate((130.8128,164.8138,195.9977)):
        y,_ = inst.render(5, pitch=pitch, params={'noise_seed':11+i},gate_off=3)
        audio += y
    assert np.max(np.abs(audio)) < 1
    with wave.open(str(args.output/'choir-demo.wav'),'wb') as f:
        f.setnchannels(2); f.setsampwidth(2); f.setframerate(48000)
        f.writeframes((audio*32767).astype('<i2').tobytes())
    (args.output/'voice-results.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(dict(preset_renders=len(results['presets']),
                          block_checks=results['block_checks'],
                          benchmark=results['single_voice_microbenchmark']),indent=2))


if __name__=='__main__':
    main()
