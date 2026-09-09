#!/usr/bin/env python3
"""Matched lazy-gate experiment; never rewrites the installed instrument.

Requires a compiler supporting scalar leaf-table reads in block-gate. Both
baseline and gated DSP use the exact same compiler, sample rate and inputs.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import time

import numpy as np

sys.dont_write_bytecode = True
from validate_kernel import Instrument, ROOT
from presets import make_presets
from build_instrument import build, OPERATOR_COUNT

HERE = Path(__file__).resolve().parent


def gate_modes(source):
    for i in range(1, OPERATOR_COUNT + 1):
        formant = f'(ff-formant phase_{i} freq_{i} center_{i} width_{i} (+ mv{i}_skirt ctl_skirt_offset) pm_{i})'
        sine = f'(sin (+ phase_{i} pm_{i}))'
        old = f'(gswitch mode_{i} {formant} {sine})'
        new = f'(gswitch mode_{i} (block-gate mode_{i} {formant}) (block-gate (eq mode_{i} 0) {sine}))'
        if source.count(old) != 1:
            raise ValueError(f'Expected exactly one operator {i} selection')
        source = source.replace(old, new)
    return source


class Prepared:
    def __init__(self, inst, params):
        self.inst = inst
        self.inputs = [np.zeros(128, np.float32) for _ in range(inst.n_in)]
        self.outputs = [np.zeros(128, np.float32) for _ in range(inst.n_out)]
        for name, value in [('pitch', 220), ('gate', 1), ('velocity', 1)]:
            if name in inst.inputs:
                self.inputs[inst.inputs[name]][:] = value
        ptr = ctypes.POINTER(ctypes.c_float)
        self.ip = (ptr * inst.n_in)(*[a.ctypes.data_as(ptr) for a in self.inputs])
        self.op = (ptr * inst.n_out)(*[a.ctypes.data_as(ptr) for a in self.outputs])
        self.memory = inst.fresh_memory()
        for name, value in params.items():
            self.memory[inst.params[name]['cellId']] = value
        self.mp = self.memory.ctypes.data_as(ctypes.c_void_p)
        self.ctx = ctypes.byref(inst.context)
        if 'trigger' in inst.inputs:
            self.inputs[inst.inputs['trigger']][0] = 1
        self.run(1)
        if 'trigger' in inst.inputs:
            self.inputs[inst.inputs['trigger']][:] = 0
        self.run(100)

    def run(self, blocks):
        fn = self.inst.process_fn
        for _ in range(blocks):
            fn(self.ip, self.op, 128, self.mp, self.ctx, None)


def matched_benchmark(instances, params, rounds=9, blocks=200):
    prepared = {name: Prepared(inst, params) for name, inst in instances.items()}
    batches = {name: [] for name in instances}
    rng = random.Random(7)
    for _ in range(rounds):
        order = list(prepared)
        rng.shuffle(order)
        for name in order:
            start = time.perf_counter_ns()
            prepared[name].run(blocks)
            batches[name].append((time.perf_counter_ns()-start)/blocks/1000)
    return {name: dict(median_us=float(np.median(values)), batches_us=values)
            for name, values in batches.items()}


def configuration_source(count, lazy):
    source = (HERE/'kernel.lisp').read_text()
    source += '\n(def pitch (in 1 @name pitch))\n'
    source += f'(param configuration @default 0 @min 0 @max {count-1})\n'
    # Each configuration has independent phase state, as a complete gated voice
    # would. This measures branch dispatch, not the complete 16-operator synth.
    for i in range(count):
        body = f'(ff-formant (* twopi (phasor pitch)) pitch {800+i*173} {200+i*7} 0.5 0)'
        if lazy:
            body = f'(block-gate (eq configuration {i}) {body})'
        source += f'(def configuration_{i} {body})\n'
    source += f'(out (selector (+ 1 configuration) {" ".join(f"configuration_{i}" for i in range(count))}) 1)\n'
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--audit-tool', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    os.environ['DGEN_BINARY_AUDIT_TOOL'] = str(args.audit_tool.resolve())
    args.output.mkdir(parents=True, exist_ok=True)
    options = dict(compiler=str(args.compiler.resolve()),
                   toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),
                   max_frames=128, sample_rate=48000)
    build(args.output/'eager', json.loads((HERE/'candidate/motion.json').read_text()), lazy_gates=False)
    baseline = (args.output/'eager/dsp.lisp').read_text()
    instances = {}
    result = dict(compiler=str(args.compiler.resolve()),
                  compiler_sha256=hashlib.sha256(args.compiler.read_bytes()).hexdigest(),
                  sample_rate=48000, block_size=128, sources={}, full_voice={}, comparisons=[])
    report = args.output/'results.json'

    def save():
        report.write_text(json.dumps(result, indent=2)+'\n')

    def check_fusion(inst):
        subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                        str(Path(inst.build_dir)/'patch.c')], check=True)

    for name, source in [('eager',baseline), ('lazy',gate_modes(baseline))]:
        folder = args.output/name
        folder.mkdir(exist_ok=True)
        (folder/'dsp.lisp').write_text(source)
        shutil.copyfile(HERE/'candidate/motion-tensor.json',folder/'motion-tensor.json')
        print(f'Compiling full voice: {name}', flush=True)
        instances[name] = Instrument(str(folder), **options)
        check_fusion(instances[name])
        result['sources'][name] = dict(sha256=hashlib.sha256(source.encode()).hexdigest(),
                                      build_dir=instances[name].build_dir)
    defaults = json.loads((HERE/'candidate/defaults.json').read_text())
    presets = {p['name']:p['params'] for p in make_presets(defaults)}
    cases = dict(formants=defaults, sine_fm=presets['Keys 1'], feedback_fm=presets['Metal 2'])
    mixed = dict(defaults)
    mixed.update({f'v{i}_mode':0 for i in range(1, OPERATOR_COUNT + 1, 2)})
    cases['mixed'] = mixed
    for name, params in cases.items():
        eager,_ = instances['eager'].render(.6, pitch=220, params=params, gate_off=.3)
        lazy,_ = instances['lazy'].render(.6, pitch=220, params=params, gate_off=.3)
        row = dict(case=name, peak_error=float(np.max(np.abs(eager-lazy))),
                   rms_error=float(np.sqrt(np.mean((eager-lazy)**2))),
                   finite=bool(np.isfinite(lazy).all()))
        result['comparisons'].append(row)
        timing = matched_benchmark(instances, params)
        result['full_voice'][name] = timing
        save()
        print(name, row, {n:round(t['median_us'],2) for n,t in timing.items()}, flush=True)
    # Mid-block note-on changes the latched waveform mode. Compare this against
    # eager execution so frozen, formant-only smoothers cannot hide stale state.
    ramps = {f'v{i}_mode':[(0,0),(.12,1),(.28,0)] for i in range(1, OPERATOR_COUNT + 1)}
    renders = [inst.render(.45,pitch=220,params=presets['Keys 1'],ramps=ramps,
                           retrig=[.123,.283])[0] for inst in instances.values()]
    result['comparisons'].append(dict(case='mode_changes_on_mid_block_note_on',
        peak_error=float(np.max(np.abs(renders[0]-renders[1]))),
        finite=bool(np.isfinite(renders[1]).all())))
    save()
    configs = {}
    for name,count,lazy in [('one',1,False),('thirty_eager',30,False),('thirty_lazy',30,True)]:
        source = configuration_source(count,lazy)
        folder = args.output/name
        folder.mkdir(exist_ok=True)
        (folder/'dsp.lisp').write_text(source)
        print(f'Compiling configuration benchmark: {name}',flush=True)
        configs[name] = Instrument(str(folder),**options)
        check_fusion(configs[name])
    result['configuration_benchmark'] = matched_benchmark(configs,{'configuration':0})
    for selected in (0,14,29):
        a,_ = configs['thirty_eager'].render(.1,params={'configuration':selected})
        b,_ = configs['thirty_lazy'].render(.1,params={'configuration':selected})
        result['comparisons'].append(dict(case=f'configuration_{selected}',
            peak_error=float(np.max(np.abs(a-b))), finite=bool(np.isfinite(b).all())))
    save()
    print(json.dumps(result['configuration_benchmark'],indent=2),flush=True)
    if not all(row['finite'] and row['peak_error'] < 1e-4 for row in result['comparisons']):
        raise RuntimeError('Gated output differs from eager output; do not deploy this experiment')


if __name__ == '__main__':
    main()
