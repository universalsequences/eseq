#!/usr/bin/env python3
"""Compare two compilers on the unchanged production causal FilterTable (macOS).

Uses the app's FFT host services, procedural magnitude tables, and native thread
CPU time. This isolates an effect; it does not predict four-worker transport CPU.
Requires numpy. All compiled artifacts and measurements remain in --out.
"""
import argparse
import ctypes as C
import hashlib
import json
import platform
from pathlib import Path
import statistics
import subprocess

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
FP = C.POINTER(C.c_float)


class Context(C.Structure):
    _fields_ = [("version", C.c_uint32), ("size", C.c_uint32),
                ("sample_rate", C.c_float), ("reserved", C.c_uint32)]


def pointers(arrays):
    return (FP * len(arrays))(*(a.ctypes.data_as(FP) for a in arrays))


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Kernel:
    def __init__(self, compiler, source, folder, stage, host, rate):
        folder.mkdir(parents=True, exist_ok=True)
        command = [str(compiler), str(source), '-o', str(folder), '--name', 'patch',
                   '--sample-rate', str(rate), '--max-frames', '512', '--voices', '1',
                   '--toolchain-root', str(stage)]
        with (folder / 'compile.log').open('w') as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        if result.returncode:
            raise RuntimeError(f'compiler failed; see {folder / "compile.log"}')
        subprocess.run(['python3', str(ROOT / 'tools/audition/check_fusion.py'),
                        str(folder / 'patch.c')], check=True)
        self.manifest = json.loads((folder / 'patch.json').read_text())
        if self.manifest['processAbi'] != 'dgen-host-abi-v1':
            raise ValueError('unsupported generated process ABI')
        self.params = {p['name']: p for p in self.manifest['params']}
        self.inputs = {p['name']: p['channel'] for p in self.manifest['inputs']}
        self.n_in = max(self.inputs.values()) + 1
        self.n_out = len(self.manifest['outputs'])
        self.library = C.CDLL(str(folder / self.manifest['dylib']))
        self.process = self.library.dgen_process_v1
        self.process.argtypes = [C.POINTER(FP), C.POINTER(FP), C.c_uint32,
                                 C.c_void_p, C.POINTER(Context), C.c_void_p]
        self.process.restype = None
        self.context = Context(1, C.sizeof(Context), rate, 0)
        self.host = host

    def memory(self, table):
        state = np.zeros(self.manifest['totalMemorySlots'], dtype=np.float32)
        for tensor in self.manifest.get('tensorInitData', []):
            offset, data = tensor['offset'], tensor['data']
            state[offset:offset + len(data)] = data
        for param in self.params.values():
            state[param['cellId']] = param.get('default', 0)
        tensor = next(t for t in self.manifest['tensors'] if t['name'] == 'table_magnitudes')
        offset = tensor['cellOffset']
        if np.prod(tensor['shape']) != table.size:
            raise ValueError('table shape differs from production contract')
        state[offset:offset + table.size] = table.ravel()
        state[self.params['mix']['cellId']] = 1
        return state

    def render(self, table, automated, chunks):
        state = self.memory(table)
        count = 24576
        # The host and compiler runtime reserve four guard samples for SIMD.
        inputs = np.zeros((self.n_in, count + 4), dtype=np.float32)
        t = np.arange(count) / self.context.sample_rate
        inputs[self.inputs['left'], :count] = .2 * np.sin(2 * np.pi * 337 * t)
        inputs[self.inputs['right'], :count] = .1 * np.cos(2 * np.pi * 1823 * t)
        inputs[self.inputs['left'], 0] += 1
        inputs[self.inputs['right'], 1024] -= .5
        outputs = np.zeros((self.n_out, count + 4), dtype=np.float32)
        cursor, call = 0, 0
        while cursor < count:
            # Events occur at identical sample positions in every partition.
            event = cursor // 512
            for name, value in controls(event, automated).items():
                state[self.params[name]['cellId']] = value
            frames = min(chunks[call % len(chunks)], count - cursor, 512 - cursor % 512)
            self.process(pointers([a[cursor:] for a in inputs]),
                         pointers([a[cursor:] for a in outputs]), frames,
                         state.ctypes.data, C.byref(self.context), self.host)
            cursor += frames
            call += 1
        if not np.isfinite(outputs).all() or np.max(np.abs(outputs)) < .01:
            raise AssertionError('effect output is nonfinite or unexpectedly silent')
        return outputs[:, :count]


def controls(block, automated):
    if not automated:
        return {'frame': .3, 'cutoff': 1000, 'resonance': .25}
    return {'frame': (block % 31) / 30,
            'cutoff': 40 * (450 ** ((block % 23) / 22)),
            'resonance': (block % 17) / 16}


def difference(actual, expected):
    delta = actual.astype(np.float64) - expected
    return {'max_abs': float(np.max(np.abs(delta))),
            'nrmse': float(np.sqrt(np.mean(delta * delta)) /
                           max(1e-12, np.sqrt(np.mean(expected.astype(np.float64) ** 2))))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', required=True, type=Path)
    parser.add_argument('--candidate', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--repetitions', type=int, default=9)
    args = parser.parse_args()
    if platform.system() != 'Darwin':
        parser.error('this harness uses the app\'s macOS FFT implementation')
    if args.repetitions < 3:
        parser.error('use at least three alternating repetitions')
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = output / 'filter_table.lisp'
    fx = ROOT / 'crates/sequencer/src/effects'
    source.write_text((fx / 'filter_table_dsp.lisp').read_text() + '\n' +
                      (fx / 'filter_table_dsp_causal.lisp').read_text())
    graph = ROOT / 'crates/sequencer/audiograph'
    support = output / 'effect_benchmark.dylib'
    subprocess.run(['cc', '-O2', '-dynamiclib', str(Path(__file__).with_name('effect_benchmark.c')),
                    str(graph / 'dgen_host_services.c'), str(graph / 'dgen_fft.c'),
                    '-framework', 'Accelerate', '-o', str(support)], check=True)
    driver = C.CDLL(str(support))
    driver.eseq_dgen_host_services_v1.restype = C.c_void_p
    host = driver.eseq_dgen_host_services_v1()
    timer = driver.benchmark_effect
    timer.restype = C.c_double
    timer.argtypes = [C.c_void_p, C.POINTER(FP), C.POINTER(FP), C.c_uint32, C.c_void_p,
                      C.POINTER(Context), C.c_uint32, C.c_void_p, C.c_uint32, C.c_void_p]
    stage = ROOT / 'crates/sequencer/tools/dgen-toolchain'
    compilers = [args.baseline.resolve(), args.candidate.resolve()]
    kernels = [Kernel(c, source, output / name, stage, host, 48000)
               for c, name in zip(compilers, ['baseline', 'candidate'])]
    rows, bins = np.arange(64)[:, None], np.arange(1025)[None, :]
    table = (0.02 + .98 * np.exp(-bins / (24 + 8 * rows)) *
             (.5 + .5 * np.cos(bins * (.02 + .0007 * rows)) ** 2)).astype(np.float32)
    report = {'source_sha256': sha(source), 'compiler_sha256': [sha(c) for c in compilers],
              'sample_rate': 48000, 'frames': 512, 'correctness': [], 'timings': {},
              'partition_diagnostics': [], 'platform': platform.platform(),
              'toolchain_lock_sha256': sha(ROOT / 'content/dgen-toolchain.lock'),
              'generated_c_sha256': [sha(output / name / 'patch.c') for name in ['baseline', 'candidate']],
              'timing_clock': 'CLOCK_THREAD_CPUTIME_ID', 'warmup_blocks': 256,
              'measured_blocks': 2048, 'discarded_repetitions': 2}
    for bank, data in [('flat', np.ones_like(table)), ('shaped', table)]:
        for automated in [False, True]:
            regular = [k.render(data, automated, [512]) for k in kernels]
            irregular = [k.render(data, automated, [1, 7, 31, 127, 256, 3]) for k in kernels]
            comparisons = {'compiler': difference(regular[1], regular[0]),
                           'irregular_compiler': difference(irregular[1], irregular[0]),
                           'baseline_partition': difference(irregular[0], regular[0]),
                           'candidate_partition': difference(irregular[1], regular[1])}
            report['correctness'].append({'bank': bank, 'automated': automated, **comparisons})
            for version, regular_audio, irregular_audio in zip(['baseline', 'candidate'], regular, irregular):
                np.save(output / f'{version}-{bank}-{automated}-regular.npy', regular_audio)
                np.save(output / f'{version}-{bank}-{automated}-irregular.npy', irregular_audio)
            (output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
            for label, metric in comparisons.items():
                if metric['max_abs'] > 2e-5 or metric['nrmse'] > 2e-5:
                    if label.endswith('_partition'):
                        # Compiler comparisons above must still pass for BOTH
                        # partitions. Report existing block-size sensitivity;
                        # never label the effect partition-invariant in that case.
                        report['partition_diagnostics'].append(
                            {'bank': bank, 'automated': automated, 'version': label, **metric})
                    else:
                        raise AssertionError(f'{bank}, automation={automated}, {label}: {metric}')
    print('Compiler output matches for static/automated controls in both partitions.', flush=True)
    print('Partition diagnostics: ' + json.dumps(report['partition_diagnostics']), flush=True)

    def measure(kernel, automated):
        state = kernel.memory(table)
        inputs = np.zeros((kernel.n_in, 512), dtype=np.float32)
        inputs[kernel.inputs['left']] = .2 * np.sin(np.arange(512) * .07)
        inputs[kernel.inputs['right']] = .1 * np.cos(np.arange(512) * .19)
        outputs = np.zeros((kernel.n_out, 512), dtype=np.float32)
        cells = np.array([kernel.params[n]['cellId'] for n in controls(0, automated)], dtype=np.uint32)
        values = np.array([list(controls(b, automated).values()) for b in range(2048)], dtype=np.float32)
        def run(blocks):
            elapsed = timer(C.cast(kernel.process, C.c_void_p), pointers(inputs), pointers(outputs),
                            512, state.ctypes.data, C.byref(kernel.context), blocks,
                            cells.ctypes.data, len(cells), values.ctypes.data)
            if elapsed < 0 or not np.isfinite(outputs).all():
                raise RuntimeError('timing clock failed or output became nonfinite')
            return elapsed / blocks * 1e6
        run(256)  # More than 2.7 seconds of audio, including initial FFT setup.
        return run(2048)

    for automated in [False, True]:
        values = [[], []]
        for repetition in range(args.repetitions + 2):
            for index in ([0, 1] if repetition % 2 == 0 else [1, 0]):
                elapsed = measure(kernels[index], automated)
                if repetition >= 2:
                    values[index].append(elapsed)
        medians = [statistics.median(v) for v in values]
        report['timings']['automated' if automated else 'static'] = {
            'thread_cpu_us': values, 'median_us': medians,
            'reduction_percent': 100 * (1 - medians[1] / medians[0])}
        print(json.dumps(report['timings'], indent=2), flush=True)
    (output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
