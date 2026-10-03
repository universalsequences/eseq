"""Compiled-instrument helpers shared by compare, verify and performance."""
import ctypes
import os
import platform
import subprocess
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT/'tools/audition'))
os.environ.setdefault('AUDITION_CACHE', str(ROOT/'.local/pm-ride2/cache'))
from audition import Instrument  # noqa: E402
from build import DEST, FIRST_KEY  # noqa: E402


def instrument(sr, block=128, source=DEST):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    inst = Instrument(str(source), compiler=os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT/'crates/sequencer/tools'/target)),
                      toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),
                      sample_rate=sr, max_frames=block, voices=1)
    audit = subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir)/'patch.c')], capture_output=True, text=True)
    if audit.returncode:
        raise RuntimeError('Generated-C fusion audit failed:\n' + audit.stdout + audit.stderr)
    return inst


def hz(note):
    return 440*2**((note - 69)/12)


def play(inst, events, seconds, params=None, mem=None, release=None):
    """One voice (the one-cymbal voicing): events = [(time s, key, velocity)].

    Every event is a retrigger of the same voice, as the host's mono
    allocation sends it: the gate stays high and the trigger pulses for one
    sample while pitch and velocity change on that sample. `release` drops
    the gate at that time. No events continues a ringing voice from `mem`
    with the gate held and no new strike. Returns (stereo audio, state).
    """
    sr = inst.sample_rate
    n = int(seconds*sr)
    blk = inst.max_frames
    mem = inst.fresh_memory() if mem is None else mem
    for name, value in (params or {}).items():
        mem[inst.params[name]['cellId']] = value
    pitch = np.zeros(n, np.float32)
    vel = np.zeros(n, np.float32)
    trig = np.zeros(n, np.float32)
    gate = np.ones(n, np.float32)
    events = sorted(events)
    for i, (t, key, v) in enumerate(events):
        s = int(round(t*sr))
        e = int(round(events[i + 1][0]*sr)) if i + 1 < len(events) else n
        pitch[s:e] = hz(key)
        vel[s:e] = v
        trig[s] = 1.0
    if events:
        first = int(round(events[0][0]*sr))
        pitch[:first], vel[:first], gate[:first] = pitch[first], vel[first], 0.0
    else:
        pitch[:] = hz(FIRST_KEY)
    if release is not None:
        gate[int(release*sr):] = 0.0
    ins = [np.zeros(blk, np.float32) for _ in range(inst.n_in)]
    outs = [np.zeros(blk, np.float32) for _ in range(inst.n_out)]
    P = ctypes.POINTER(ctypes.c_float)
    inptrs = (P*inst.n_in)(*[a.ctypes.data_as(P) for a in ins])
    outptrs = (P*inst.n_out)(*[a.ctypes.data_as(P) for a in outs])
    y = np.zeros((n, inst.n_out), np.float32)
    channels = {'pitch': pitch, 'velocity': vel, 'trigger': trig, 'gate': gate}
    for b in range(0, n, blk):
        frames = min(blk, n - b)
        for name, data in channels.items():
            if name in inst.inputs:
                ins[inst.inputs[name]][:frames] = data[b:b + frames]
        inst.process_fn(inptrs, outptrs, frames, mem.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
        for c in range(inst.n_out):
            y[b:b + frames, c] = outs[c][:frames]
    return y, mem
