"""Deterministic production ABI driver with off-grid events and variable blocks."""
import ctypes
import numpy as np

def stream(inst, frames=24000, block=128, onset=137, idle=False, gate_only=False,
           events=None, params=None, key=77, vel=.8, gate_end=12003):
    state = inst.fresh_memory()
    for name, value in (params or {}).items():
        state[inst.params[name]['cellId']] = value
    inputs = [np.zeros(frames, dtype=np.float32) for _ in range(inst.n_in)]
    outputs = [np.zeros(frames, dtype=np.float32) for _ in range(inst.n_out)]
    inputs[inst.inputs['pitch']][:] = 440*2**((key-69)/12)
    if not idle:
        inputs[inst.inputs['gate']][onset:gate_end] = 1
        inputs[inst.inputs['velocity']][onset:gate_end] = vel
        if not gate_only:
            inputs[inst.inputs['trigger']][onset] = 1
    events = events or {}
    boundaries = sorted([*events, frames])
    offset = 0
    while offset < frames:
        for name, value in events.get(offset, {}).items():
            state[inst.params[name]['cellId']] = value
        count = min(block, next(b for b in boundaries if b > offset)-offset)
        def pointers(arrays):
            return (ctypes.POINTER(ctypes.c_float)*len(arrays))(*[
                a[offset:].ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in arrays])
        inst.process_fn(pointers(inputs), pointers(outputs), count,
                        state.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
        offset += count
    return np.array(outputs).T, state
