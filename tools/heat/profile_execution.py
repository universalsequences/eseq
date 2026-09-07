#!/usr/bin/env python3
"""Compare two compiled Heat ABI-v1 artifacts with native (not Python-loop) timing.

Compile both with --voices 12 --max-frames 512 --sample-rate 48000 --name patch.
Requires numpy and a C compiler. Artifact directories contain patch.json and
patch.dylib (or patch.so). This measures one host voice, not the callback meter.
"""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tempfile

import numpy as np

P = C.POINTER(C.c_float)


class Context(C.Structure):
    _fields_ = [("abi", C.c_uint32), ("size", C.c_uint32),
                ("rate", C.c_float), ("reserved", C.c_uint32)]


class Patch:
    def __init__(self, path):
        self.path = path.resolve()
        self.manifest = json.loads((path / "patch.json").read_text())
        if self.manifest.get("processAbi") != "dgen-host-abi-v1":
            raise ValueError("This profiler requires dgen-host-abi-v1")
        self.binary = path / ("patch.dylib" if platform.system() == "Darwin" else "patch.so")
        self.lib = C.CDLL(str(self.binary.resolve()))
        self.fn = self.lib.dgen_process_v1
        self.fn.argtypes = [C.POINTER(P), C.POINTER(P), C.c_uint32,
                            C.c_void_p, C.c_void_p, C.c_void_p]
        self.fn.restype = None
        self.params = {p["name"]: p for p in self.manifest["params"]}
        self.names = {i["name"]: i["channel"] for i in self.manifest["inputs"]}
        self.ctx = Context(1, C.sizeof(Context), 48000, 0)

    def setup(self, params):
        unknown = set(params) - self.params.keys()
        if unknown:
            raise ValueError(f"Unknown parameters: {unknown}")
        self.mem = np.zeros(max(1024, self.manifest["totalMemorySlots"]), np.float32)
        for p in self.params.values():
            self.mem[p["cellId"]] = params.get(p["name"], p["default"])
        self.ins = [np.zeros(516, np.float32) for _ in self.manifest["inputs"]]
        self.outs = [np.zeros(516, np.float32) for _ in self.manifest["outputs"]]
        self.ip = (P * len(self.ins))(*[a.ctypes.data_as(P) for a in self.ins])
        self.op = (P * len(self.outs))(*[a.ctypes.data_as(P) for a in self.outs])
        self.mp = self.mem.ctypes.data_as(C.c_void_p)
        self.ins[self.names["pitch"]][:] = 220
        self.ins[self.names["velocity"]][:] = 1

    def call(self, frames):
        self.fn(self.ip, self.op, frames, self.mp, C.byref(self.ctx), None)

    def trace(self, params, block, events=None, modulation=False):
        self.setup(params)
        data = []
        for offset in range(0, 48000, block):
            frames = min(block, 48000 - offset)
            for name, value in (events or {}).get(offset, {}).items():
                self.mem[self.params[name]["cellId"]] = value
            self.ins[self.names["gate"]][:] = int(offset < 24000)
            for name in ("trigger", "note_on"):
                self.ins[self.names[name]][:] = 0
                self.ins[self.names[name]][0] = int(offset == 0)
            if modulation:
                self.ins[self.names["mod1"]][:frames] = np.sin(
                    (offset + np.arange(frames)) * (2 * np.pi * 5 / 48000))
            self.call(frames)
            data.append(np.stack([a[:frames].copy() for a in self.outs]))
        result = np.concatenate(data, axis=1)
        if not np.isfinite(result).all() or not np.isfinite(self.mem).all():
            raise AssertionError("Non-finite Heat audio/state")
        return result

    def identity(self):
        return {"directory": str(self.path), "sha256": {
            p.name: hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (self.path / "patch.c", self.path / "patch.json", self.binary)}}


def check_modulation(patches):
    results = {}
    for block in (128, 512):
        active = "__mod__volume_db__active"
        depth = "__mod__volume_db__depth__slot1"
        events = {block * 8: {active: 1}, block * 24: {active: 0},
                  block * 32: {active: 1}}
        before, after = [p.trace({depth: 12}, block, events, modulation=True) for p in patches]
        error = float(abs(before - after).max())
        unassigned = patches[1].trace({depth: 12}, block, modulation=True)
        effect = float(abs(after - unassigned).max())
        if error > 2e-6 or effect < .001:
            raise AssertionError(f"Modulation assignment regression: error={error}, effect={effect}")
        results[f"modulation-assign-remove/{block}"] = {"max_error": error, "effect_peak": effect}
    return results


HARNESS = r"""
#include <stdint.h>
#include <time.h>
typedef void (*process_fn)(float**, float**, uint32_t, void*, void*, void*);
double bench(process_fn fn, float** in, float** out, uint32_t frames,
             void* memory, void* context, int count) {
    struct timespec start, end;
    clock_gettime(CLOCK_MONOTONIC, &start);
    for (int i = 0; i < count; ++i) fn(in, out, frames, memory, context, 0);
    clock_gettime(CLOCK_MONOTONIC, &end);
    return (end.tv_sec - start.tv_sec) + (end.tv_nsec - start.tv_nsec) * 1e-9;
}
"""


def run(args):
    patches = [Patch(args.baseline), Patch(args.candidate)]
    result = {"scope": "One host voice; 48 kHz; 12-voice compilation; native wall time",
              "machine": platform.platform(),
              "baseline": patches[0].identity(), "candidate": patches[1].identity(),
              "comparisons": {}, "timings": {}}
    cases = {"default": {}, "unison4": {"unison_voices": 4},
             "all-on": {"unison_voices": 4, "osc2_enabled": 1, "noise_enabled": 1,
                        "lfo1_enabled": 1, "lfo2_enabled": 1, "osc1_sub_sync": 1,
                        "osc2_sub_sync": 1, "filter1_drive": 3, "filter2_drive": 3,
                        "filter1_mode": 1, "filter2_mode": 1}}
    for wave in (0.4, 0.6, 1.6, 2.6):
        cases[f"fractional-wave-{wave}"] = {"osc1_wave": wave}
    for block in (128, 512):
        for name, params in cases.items():
            before, after = [p.trace(params, block) for p in patches]
            error = float(abs(before - after).max())
            if error > 2e-6:
                raise AssertionError(f"{name}/{block} changed audio: {error}")
            result["comparisons"][f"{name}/{block}"] = {
                "max_error": error, "peak": float(abs(after).max()),
                "tail_peak": float(abs(after[:, -1024:]).max())}
        # Exercise actual state freezing and reactivation, finishing with all
        # sources disabled. Unison fades and delayed onsets remain in this path.
        events = {block * 8: {"unison_voices": 4, "osc2_enabled": 1},
                  block * 16: {"unison_voices": 1, "osc1_enabled": 0, "osc2_enabled": 0},
                  block * 24: {"unison_voices": 4, "osc1_enabled": 1},
                  block * 32: {"osc1_enabled": 0, "osc2_enabled": 0}}
        toggled = patches[1].trace({}, block, events)
        tail = float(abs(toggled[:, -1024:]).max())
        if tail > 1e-7:
            raise AssertionError(f"Toggle sequence left a tail: {tail}")
        result["comparisons"][f"toggle/{block}"] = {"tail_peak": tail}
    result["comparisons"].update(check_modulation(patches))
    with tempfile.TemporaryDirectory(prefix="heat-native-timing-") as folder:
        source = Path(folder) / "timing.c"
        source.write_text(HARNESS)
        library = Path(folder) / "timing.so"
        subprocess.run(["cc", "-O2", "-shared", "-fPIC", str(source), "-o", str(library)], check=True)
        native = C.CDLL(str(library))
        bench = native.bench
        bench.argtypes = [C.c_void_p, C.POINTER(P), C.POINTER(P), C.c_uint32,
                          C.c_void_p, C.c_void_p, C.c_int]
        bench.restype = C.c_double
        for block in (128, 512):
            for patch in patches:
                patch.setup({})
                patch.ins[patch.names["gate"]][:] = 1
                for name in ("trigger", "note_on"):
                    patch.ins[patch.names[name]][0] = 1
                patch.call(block)
                for name in ("trigger", "note_on"):
                    patch.ins[patch.names[name]][0] = 0
                for _ in range(100):
                    patch.call(block)
            times = [[], []]
            for trial in range(args.trials):
                for i in ([0, 1] if trial % 2 == 0 else [1, 0]):
                    patch = patches[i]
                    count = 48000 * 2 // block
                    elapsed = bench(patch.fn, patch.ip, patch.op, block, patch.mp,
                                    C.byref(patch.ctx), count)
                    times[i].append(elapsed / count * 1e6)
            speedup = float(np.median(times[0]) / np.median(times[1]))
            result["timings"][str(block)] = {"baseline_us": times[0], "candidate_us": times[1],
                                              "speedup": speedup}
            print(f"{block} frames: {speedup:.2f}x ({np.median(times[0]):.1f} -> {np.median(times[1]):.1f} us)", flush=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    if any(t["speedup"] < args.min_speedup for t in result["timings"].values()):
        raise AssertionError(f"Speedup below {args.min_speedup}x; inspect raw timings and system load")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--trials", type=int, default=9)
    parser.add_argument("--min-speedup", type=float, default=4)
    run(parser.parse_args())
