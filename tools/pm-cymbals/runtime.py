"""Production-ABI event rendering and the same native timer as PM gamelan."""
import ctypes as C
import platform
import subprocess

import numpy as np
from common import ROOT


def stream(inst, seconds=1., block=128, params=None, hits=None, events=None, gate_only=False):
    count = round(seconds*inst.sample_rate)
    state = inst.fresh_memory()
    for name, value in (params or {}).items():
        state[inst.params[name]['cellId']] = value
    inputs = np.zeros((inst.n_in, count), np.float32)
    outputs = np.zeros((inst.n_out, count), np.float32)
    inputs[inst.inputs['pitch']] = 261.625565
    for offset, velocity in (hits or {}).items():
        inputs[inst.inputs['velocity'], offset:] = velocity
        inputs[inst.inputs['gate'], offset:] = 1
        if not gate_only:
            inputs[inst.inputs['trigger'], offset] = 1
    events = events or {}
    boundaries = sorted({*events, count})
    offset = 0
    while offset < count:
        for name, value in events.get(offset, {}).items():
            state[inst.params[name]['cellId']] = value
        n = min(block, next(b for b in boundaries if b > offset)-offset)
        def pointers(arrays):
            return (C.POINTER(C.c_float)*len(arrays))(*[
                a[offset:].ctypes.data_as(C.POINTER(C.c_float)) for a in arrays])
        inst.process_fn(pointers(inputs), pointers(outputs), n,
                        state.ctypes.data_as(C.c_void_p), C.byref(inst.context), None)
        offset += n
    return outputs.T, state


def timer(output):
    library = output/'benchmark.dylib'
    # Reuse the gamelan C loop so timings have identical trigger cadence and
    # exclude allocation, Python, compilation, and file I/O in both families.
    subprocess.run(['cc', '-O2', '-dynamiclib' if platform.system() == 'Darwin' else '-shared',
                    '-fPIC', str(ROOT/'tools/pm-gamelan/benchmark.c'), '-o', str(library)], check=True)
    lib = C.CDLL(str(library))
    run = lib.run
    run.restype = C.c_double
    run.argtypes = [C.c_void_p]*3+[C.c_uint32]+[C.c_void_p]*3+[C.c_uint32]
    def measure(inst, params=None, seconds=4.):
        frames = inst.max_frames
        inputs = np.zeros((inst.n_in, frames), np.float32)
        outputs = np.zeros((inst.n_out, frames), np.float32)
        state = inst.fresh_memory()
        for name, value in (params or {}).items():
            state[inst.params[name]['cellId']] = value
        for name, value in [('pitch', 698.456463), ('velocity', .8), ('gate', 1)]:
            inputs[inst.inputs[name]] = value
        def pointers(arrays):
            return (C.c_void_p*len(arrays))(*[a.ctypes.data for a in arrays])
        blocks = round(seconds*inst.sample_rate/frames)
        elapsed = run(C.cast(inst.process_fn, C.c_void_p), pointers(inputs), pointers(outputs),
                      frames, state.ctypes.data, C.byref(inst.context),
                      inputs[inst.inputs['trigger']].ctypes.data, blocks)
        assert np.isfinite(state).all() and np.isfinite(outputs).all()
        return elapsed/blocks*1e6
    return measure
