#!/usr/bin/env python3
"""CPU of the Elements instrument (DGen) against the original elements::Part
(C++ -O3), per scenario, as % of one core for one voice playing note 48.

    python3 tools/bench.py [INSTRUMENT_DIR]   (default: the factory instrument)

Both sides run as plain C processes (ref/elbench, tools/dgdrv.c) under
/usr/bin/time -l; the cycles of a 0-second run (load, init) are subtracted.
Cycle counts barely move with machine load, unlike wall-clock timings.
The module runs at 32 kHz; the instrument runs at the host rate, so the 32k
column compares the same work and the 48k column is what the host pays.
Env: SR=32000,48000  SEC=20  ONLY=strike,ominous  GHZ=3.228 (to print % core).
Needs ref/elbench: EURORACK=... ref/build.sh.
"""
import os, re, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

A = elh.A
REF = os.path.join(elh.ROOT, "ref", "elbench")
SEC = float(os.environ.get("SEC", 20))
SRS = [float(s) for s in os.environ.get("SR", "32000,48000").split(",")]
GHZ = float(os.environ.get("GHZ", 3.228)) * 1e9
NOTE = 48.0
# name, patch shared by both, C++-only args, instrument-only params, idle
# (no notes: elbench gate=-1, dgdrv idle)
SCENARIOS = [
    ("strike", {}, {}, {}, False),
    ("bow", dict(bow=0.8, strike=0.0), {}, {}, False),
    ("blow", dict(blow=0.8, strike=0.0), {}, {}, False),
    ("all-exc", dict(bow=0.6, blow=0.6, strike=0.6), {}, {}, False),
    ("dry", dict(space=0.0), {}, {}, False),
    ("string", {}, dict(model=1), dict(model=1), False),
    ("strings", {}, dict(model=2), dict(model=2), False),
    ("ominous", {}, dict(easter=1), dict(model=3), False),
    ("idle", {}, {}, {}, True),
]


def cycles(args):
    r = subprocess.run(["/usr/bin/time", "-l"] + args, capture_output=True, text=True, check=True)
    return int(re.search(r"(\d+)\s+cycles elapsed", r.stderr).group(1))


def per_second(command):
    """Cycles per second of audio: best of 3 runs minus best of 2 empty runs."""
    return (min(cycles(command(SEC)) for _ in range(3)) - min(cycles(command(0)) for _ in range(2))) / SEC


def driver():
    out = os.path.join(elh.WORK, "dgdrv")
    src = os.path.join(elh.HERE, "dgdrv.c")
    if not os.path.exists(out) or os.path.getmtime(out) < os.path.getmtime(src):
        os.makedirs(elh.WORK, exist_ok=True)
        subprocess.run(["clang", "-O2", "-o", out, src], check=True)
    return out


def cpp(args, idle):
    args = dict(args, gate=-1) if idle else args
    return per_second(lambda s: [REF, str(s), f"note={NOTE}"] + [f"{k}={v}" for k, v in args.items()])


def dgen(drv, inst, params, idle):
    mem = inst.fresh_memory()
    # level_db -12 cancels the shell's +12 dB makeup for the resonator models
    for k, v in {"level_db": -12.0, **params}.items():
        mem[inst.params[k]["cellId"]] = v
    path = os.path.join(elh.WORK, "bench_mem.raw")
    mem.tofile(path)
    i = inst.inputs
    dylib = os.path.join(inst.build_dir, "patch.dylib")
    hz = 440 * 2 ** ((NOTE - 69) / 12)
    return per_second(lambda s: [drv, dylib, path, str(inst.n_in), str(inst.n_out), str(i["gate"]),
                                 str(i["pitch"]), str(i.get("velocity", -1)), str(i.get("trigger", -1)), str(hz),
                                 str(inst.sample_rate), str(inst.max_frames), str(s), str(int(idle))])


def main():
    if not os.path.exists(REF):
        sys.exit("build ref/elbench first: EURORACK=... ref/build.sh")
    inst_dir = sys.argv[1] if len(sys.argv) > 1 else elh.INSTRUMENT
    drv = driver()
    insts = {sr: A.Instrument(inst_dir, sample_rate=sr, max_frames=128) for sr in SRS}
    only = os.environ.get("ONLY")
    print(f"{'scenario':9s}  C++@32k" + "".join(f"  DGen@{sr / 1000:g}k  ratio" for sr in SRS), flush=True)
    for name, shared, cpp_args, dgen_params, idle in SCENARIOS:
        if only and name not in only.split(","):
            continue
        c = cpp({**shared, **cpp_args}, idle)
        row = f"{name:9s}  {100 * c / GHZ:6.2f}%"
        for sr in SRS:
            d = dgen(drv, insts[sr], {**shared, **dgen_params}, idle)
            row += f"  {100 * d / GHZ:7.2f}%  {d / c:4.1f}x"
        print(row, flush=True)


if __name__ == "__main__":
    main()
