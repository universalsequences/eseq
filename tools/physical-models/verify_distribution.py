#!/usr/bin/env python3
"""Standalone compiler/ABI checks requiring only the Python standard library.

Sources must already include the host preamble (and materialized library macros).
This keeps the Linux packaging check independent of a Rust or NumPy install.
"""
import argparse
import ctypes as C
import hashlib
import json
import math
import platform
import subprocess
from pathlib import Path


class Context(C.Structure):
    _fields_ = [('abi_version', C.c_uint32), ('struct_size', C.c_uint32),
                ('sample_rate', C.c_float), ('reserved', C.c_uint32)]


def render(library, manifest, parts):
    count, capacity = 8192, 512
    memory = (C.c_float*manifest['totalMemorySlots'])()
    for tensor in manifest.get('tensorInitData', []):
        for index, value in enumerate(tensor['data']):
            memory[tensor['offset']+index] = value
    for param in manifest['params']:
        memory[param['cellId']] = param.get('default', 0.)
    inputs = [(C.c_float*capacity)() for _ in manifest['inputs']]
    outputs = [(C.c_float*capacity)() for _ in manifest['outputs']]
    names = {item['name']: item['channel'] for item in manifest['inputs']}
    input_ptrs = (C.POINTER(C.c_float)*len(inputs))(*inputs)
    output_ptrs = (C.POINTER(C.c_float)*len(outputs))(*outputs)
    context = Context(1, C.sizeof(Context), 48000., 0)
    process = library.dgen_process_v1
    process.argtypes = [C.POINTER(C.POINTER(C.c_float))]*2 + [C.c_uint32, C.c_void_p, C.POINTER(Context), C.c_void_p]
    process.restype = None
    result, offset, part = [], 0, 0
    while offset < count:
        frames = min(parts[part % len(parts)], count-offset)
        for frame in range(frames):
            sample = offset+frame
            for name, value in [('pitch', 261.625565), ('velocity', .8),
                                ('gate', float(sample >= 137)),
                                ('trigger', float(sample in [137, 138, 511, 512, 1021]))]:
                inputs[names[name]][frame] = value
        process(input_ptrs, output_ptrs, frames, memory, C.byref(context), None)
        for frame in range(frames):
            result.extend(output[frame] for output in outputs)
        offset += frames
        part += 1
    assert all(math.isfinite(value) for value in memory), 'non-finite DSP state'
    assert all(math.isfinite(value) for value in result), 'non-finite audio'
    assert max(map(abs, result)) < 16, 'unbounded audio'
    assert max(map(abs, result)) > .0001, 'silent model'
    assert not any(result[:137*len(outputs)]), 'sound before first trigger'
    return result


def main():
    parser = argparse.ArgumentParser()
    for name in ['compiler', 'toolchain', 'sources', 'output']:
        parser.add_argument('--'+name, type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    for source in sorted(args.sources.glob('*.lisp')):
        directory = args.output/source.stem
        directory.mkdir(exist_ok=True)
        compiled = subprocess.run([str(args.compiler), str(source), '-o', str(directory),
            '--name', 'patch', '--sample-rate', '48000', '--max-frames', '512',
            '--voices', '1', '--toolchain-root', str(args.toolchain)], capture_output=True, text=True)
        (directory/'compile.log').write_text(compiled.stdout+compiled.stderr)
        assert compiled.returncode == 0, (source.name, compiled.stdout[-2000:], compiled.stderr[-2000:])
        manifest = json.loads((directory/'patch.json').read_text())
        assert manifest['processAbi'] == 'dgen-host-abi-v1'
        suffix = '.dylib' if platform.system() == 'Darwin' else '.so'
        library = C.CDLL(str(directory/('patch'+suffix)))
        baseline = render(library, manifest, [512])
        errors = []
        for parts in [[1], [7, 31, 128, 17]]:
            actual = render(library, manifest, parts)
            error = max(abs(a-b) for a, b in zip(baseline, actual))
            assert error < 2e-5, (source.name, parts, error)
            errors.append(error)
        row = dict(source=source.name, max_partition_error=max(errors),
            source_sha256=hashlib.sha256(source.read_bytes()).hexdigest())
        rows.append(row)
        print(json.dumps(row), flush=True)
    assert rows, 'no prepared instrument sources'
    report = dict(platform=platform.platform(), compiler_sha256=hashlib.sha256(args.compiler.read_bytes()).hexdigest(), cases=rows)
    (args.output/'results.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
