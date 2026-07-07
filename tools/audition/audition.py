#!/usr/bin/env python3
"""Audition harness for folder custom instruments (dsp.lisp).

Compiles an instrument's dsp.lisp (with the shared INSTRUMENT_PREAMBLE from
lisp_host.rs) through DGenLisp, loads the resulting dylib via ctypes, and
drives the real process() so DSP behavior can be measured or rendered to WAV
without opening the app.

Library use:
    from audition import Instrument, partials, t60, report
    inst = Instrument("crates/sequencer/instruments/drums/membrane-tabla")
    y = inst.render(2.0, pitch=220.0, params={"stroke": 1.0})
    report("na", y)

CLI use (from repo root):
    python3 tools/audition/audition.py crates/sequencer/instruments/drums/membrane-tabla \
        --pitch a2 --set stroke=1.0 --wav /tmp/na.wav

See docs/instrument-audition-harness.md for the full method.
"""

import argparse
import ctypes
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import wave

import numpy as np

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
LISP_HOST = os.path.join(REPO_ROOT, "crates", "sequencer", "src", "lisp_host.rs")
DGEN_ROOT = os.environ.get("DGEN_ROOT", os.path.expanduser("~/code/swift/dgen"))
CACHE_ROOT = os.environ.get(
    "AUDITION_CACHE", os.path.expanduser("~/.cache/eseq-audition"))
EXPECTED_ABI = "dgen-c-v2-host-sample-rate"

NOTE_OFFSETS = {"c": -9, "d": -7, "e": -5, "f": -4, "g": -2, "a": 0, "b": 2}


def note_to_hz(spec):
    """Accept a Hz number ('220', 220.0) or a note name ('a2', 'c#3', 'eb4')."""
    if isinstance(spec, (int, float)):
        return float(spec)
    m = re.fullmatch(r"([a-gA-G])([#b]?)(-?\d)", spec.strip())
    if not m:
        return float(spec)
    semis = NOTE_OFFSETS[m.group(1).lower()]
    semis += {"#": 1, "b": -1, "": 0}[m.group(2)]
    semis += (int(m.group(3)) - 4) * 12
    return 440.0 * 2.0 ** (semis / 12.0)


def extract_preamble():
    """Pull INSTRUMENT_PREAMBLE (a raw string) out of lisp_host.rs."""
    src = open(LISP_HOST).read()
    start = src.index('const INSTRUMENT_PREAMBLE: &str = r#"')
    start = src.index('r#"', start) + 3
    end = src.index('"#', start)
    return src[start:end]


class Instrument:
    def __init__(self, path, sample_rate=48000.0, max_frames=128, voices=1,
                 verbose=False):
        """path: instrument folder or a dsp.lisp file."""
        path = os.path.abspath(path)
        self.dsp_path = path if path.endswith(".lisp") else os.path.join(path, "dsp.lisp")
        if not os.path.exists(self.dsp_path):
            raise FileNotFoundError(self.dsp_path)
        self.asset_dir = os.path.dirname(self.dsp_path)
        self.sample_rate = float(sample_rate)
        self.max_frames = int(max_frames)
        self.voices = int(voices)
        self.verbose = verbose
        self._compile()
        self._load()

    # -- compile ------------------------------------------------------------

    def _cache_key(self):
        h = hashlib.sha256()
        h.update(extract_preamble().encode())
        h.update(open(self.dsp_path, "rb").read())
        # Tensor default-files are BAKED into the dylib at compile time, so
        # asset contents must be part of the cache key.
        for name in sorted(os.listdir(self.asset_dir)):
            if name.endswith((".json", ".wav")):
                h.update(name.encode())
                h.update(open(os.path.join(self.asset_dir, name), "rb").read())
        h.update(f"{self.sample_rate}|{self.max_frames}|{self.voices}".encode())
        return h.hexdigest()[:16]

    def _compile(self):
        self.build_dir = os.path.join(CACHE_ROOT, self._cache_key())
        manifest_path = os.path.join(self.build_dir, "patch.json")
        if os.path.exists(manifest_path):
            if self.verbose:
                print(f"[audition] cached build {self.build_dir}", file=sys.stderr)
            return
        os.makedirs(self.build_dir, exist_ok=True)
        combined = os.path.join(self.build_dir, "combined.lisp")
        with open(combined, "w") as f:
            f.write(extract_preamble())
            f.write("\n")
            f.write(open(self.dsp_path).read())
        cmd = ["swift", "run", "DGenLisp", combined,
               "-o", self.build_dir, "--name", "patch",
               "--sample-rate", str(self.sample_rate),
               "--max-frames", str(self.max_frames),
               "--voices", str(self.voices),
               "--asset-base", self.asset_dir]
        if self.verbose:
            print(f"[audition] compiling: {' '.join(cmd)}", file=sys.stderr)
        r = subprocess.run(cmd, cwd=DGEN_ROOT, capture_output=True, text=True)
        if r.returncode != 0 or not os.path.exists(manifest_path):
            raise RuntimeError(
                f"DGenLisp compile failed (exit {r.returncode}):\n{r.stderr[-4000:]}")

    # -- load ---------------------------------------------------------------

    def _load(self):
        self.manifest = json.load(open(os.path.join(self.build_dir, "patch.json")))
        abi = self.manifest.get("processAbi")
        if abi != EXPECTED_ABI:
            raise RuntimeError(f"unexpected process ABI {abi!r}; this harness "
                               f"drives {EXPECTED_ABI!r} — update tools/audition")
        self.params = {p["name"]: p for p in self.manifest["params"]}
        self.inputs = {i["name"]: i["channel"] for i in self.manifest["inputs"]}
        self.n_in = len(self.manifest["inputs"])
        self.n_out = len(self.manifest["outputs"])
        self.lib = ctypes.CDLL(os.path.join(self.build_dir, "patch.dylib"))
        self.lib.process.argtypes = (
            [ctypes.POINTER(ctypes.POINTER(ctypes.c_float))] * 2
            + [ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_float])

    def fresh_memory(self):
        """State array with tensor init data and every param at its default."""
        mem = np.zeros(self.manifest["totalMemorySlots"], dtype=np.float32)
        for t in self.manifest.get("tensorInitData", []):
            data = np.array(t["data"], dtype=np.float32)
            mem[t["offset"]:t["offset"] + len(data)] = data
        # Params default to 0 in raw memory; every default must be written or
        # the patch runs in a nonsense state.
        for p in self.manifest["params"]:
            mem[p["cellId"]] = p.get("default", 0.0)
        return mem

    # -- render -------------------------------------------------------------

    def render(self, seconds=1.5, pitch=440.0, vel=1.0, params=None,
               ramps=None, retrig=None, gate_off=None, mem=None):
        """Render `seconds` of audio; returns float32 array (n,) mono or (n, ch).

        pitch    : Hz or note name; drives the 'pitch' input channel
        params   : {name: value} written once into memory before rendering
        ramps    : {name: [(t, v), ...]} piecewise-linear, updated per block.
                   Names may be param names (p-locks) or input-channel names
                   ('pitch', 'mod1', ...).
        retrig   : list of times (s) to pulse the 'trigger' input (t=0 always fires)
        gate_off : time (s) to drop the 'gate' input (None = held throughout)
        mem      : reuse a state array from a previous render (default: fresh)
        """
        sr = self.sample_rate
        n = int(seconds * sr)
        blk = self.max_frames
        if mem is None:
            mem = self.fresh_memory()
        for name, val in (params or {}).items():
            mem[self.params[name]["cellId"]] = val

        ins = [np.zeros(blk, dtype=np.float32) for _ in range(self.n_in)]
        outs = [np.zeros(blk, dtype=np.float32) for _ in range(self.n_out)]
        if "pitch" in self.inputs:
            ins[self.inputs["pitch"]][:] = note_to_hz(pitch)
        if "velocity" in self.inputs:
            ins[self.inputs["velocity"]][:] = vel
        inptrs = (ctypes.POINTER(ctypes.c_float) * self.n_in)(
            *[a.ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in ins])
        outptrs = (ctypes.POINTER(ctypes.c_float) * self.n_out)(
            *[a.ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in outs])

        trig_samples = {0} | {int(t * sr) for t in (retrig or [])}
        gate_off_sample = None if gate_off is None else int(gate_off * sr)
        trig_ch = self.inputs.get("trigger")
        gate_ch = self.inputs.get("gate")

        def ramp_value(points, t):
            pts = sorted(points)
            if t <= pts[0][0]:
                return pts[0][1]
            for (t0, v0), (t1, v1) in zip(pts, pts[1:]):
                if t < t1:
                    return v0 + (v1 - v0) * (t - t0) / max(t1 - t0, 1e-9)
            return pts[-1][1]

        y = np.zeros((n, self.n_out), dtype=np.float32)
        for b in range(0, n, blk):
            frames = min(blk, n - b)
            t = b / sr
            if gate_ch is not None:
                if gate_off_sample is None:
                    ins[gate_ch][:] = 1.0
                else:
                    idx = np.arange(b, b + blk)
                    ins[gate_ch][:] = (idx < gate_off_sample).astype(np.float32)
            if trig_ch is not None:
                ins[trig_ch][:] = 0.0
                for ts in trig_samples:
                    if b <= ts < b + frames:
                        ins[trig_ch][ts - b] = 1.0
            for name, points in (ramps or {}).items():
                v = ramp_value(points, t)
                if name in self.params:
                    mem[self.params[name]["cellId"]] = v
                elif name in self.inputs:
                    ins[self.inputs[name]][:] = v
                else:
                    raise KeyError(f"ramp target {name!r} is neither a param "
                                   f"nor an input channel")
            self.lib.process(inptrs, outptrs, frames,
                             mem.ctypes.data_as(ctypes.c_void_p),
                             None, ctypes.c_float(sr))
            for ch in range(self.n_out):
                y[b:b + frames, ch] = outs[ch][:frames]
        return (y[:, 0], mem) if self.n_out == 1 else (y, mem)


# -- analysis helpers --------------------------------------------------------

def partials(y, sr=48000.0, nmax=8, fmin=60.0, fmax=4000.0, skip=0.05):
    """Spectral peaks of the post-attack tail, sorted by frequency: [(amp, hz)]."""
    seg = np.asarray(y)[int(skip * sr):]
    if len(seg) < 256:
        return []
    w = np.abs(np.fft.rfft(seg * np.hanning(len(seg))))
    freqs = np.fft.rfftfreq(len(seg), 1 / sr)
    pk = [(w[i], freqs[i]) for i in range(2, len(w) - 2)
          if fmin <= freqs[i] <= fmax
          and w[i] > w[i - 1] and w[i] > w[i + 1] and w[i] > 0.02 * w.max()]
    pk.sort(reverse=True)
    out = []
    for a, f in pk:
        if all(abs(f - f2) > 8 for _, f2 in out):
            out.append((a, f))
        if len(out) >= nmax:
            break
    out.sort(key=lambda x: x[1])
    return out


def t60(y, sr=48000.0):
    """Decay time extrapolated from the -40 dB point of the peak envelope."""
    env = np.abs(np.asarray(y))
    hop = 480
    n = len(env) - len(env) % hop
    if n < hop:
        return None
    db = 20 * np.log10(env[:n].reshape(-1, hop).max(axis=1) + 1e-12)
    peak, ip = db.max(), db.argmax()
    for i in range(ip, len(db)):
        if db[i] < peak - 40:
            return (i - ip) * hop / sr * 1.5
    return None


def report(tag, y, sr=48000.0):
    """Print peak / NaN / T60 / partial ratios for a rendered signal."""
    y = np.asarray(y)
    pk = partials(y, sr)
    line = f"[{tag}] peak={np.abs(y).max():.3f} nan={bool(np.isnan(y).any())} t60={t60(y, sr)}"
    if pk:
        f0 = pk[0][1]
        amax = max(a for a, _ in pk)
        ratios = ", ".join(
            f"{f / f0:.2f}({f:.0f}Hz,{20 * math.log10(a / amax + 1e-12):.0f}dB)"
            for a, f in pk)
        line += f" partials f/f0: {ratios}"
    print(line)
    return pk


def write_wav(path, y, sr=48000.0):
    y = np.asarray(y)
    if y.ndim == 1:
        y = y[:, None]
    peak = np.abs(y).max()
    if peak > 1.0:
        y = y / peak
    pcm = (np.clip(y, -1, 1) * 32767).astype(np.int16)
    with wave.open(path, "wb") as f:
        f.setnchannels(y.shape[1])
        f.setsampwidth(2)
        f.setframerate(int(sr))
        f.writeframes(pcm.tobytes())


# -- CLI ----------------------------------------------------------------------

def parse_ramp(spec):
    """'press=0.2:0,0.8:1' -> ('press', [(0.2, 0.0), (0.8, 1.0)])"""
    name, pts = spec.split("=", 1)
    points = [(float(t), float(v)) for t, v in
              (pair.split(":") for pair in pts.split(","))]
    return name, points


def main():
    ap = argparse.ArgumentParser(
        description="Audition harness for folder custom instruments (dsp.lisp).")
    ap.add_argument("instrument", help="instrument folder or dsp.lisp path")
    ap.add_argument("--seconds", type=float, default=1.5)
    ap.add_argument("--pitch", default="440", help="Hz or note name like a2, c#3")
    ap.add_argument("--vel", type=float, default=1.0)
    ap.add_argument("--set", dest="sets", action="append", default=[],
                    metavar="NAME=VALUE", help="set a param (repeatable)")
    ap.add_argument("--ramp", action="append", default=[],
                    metavar="NAME=T:V,T:V", help="ramp a param or input over time")
    ap.add_argument("--retrig", default="", help="comma-separated retrigger times (s)")
    ap.add_argument("--gate-off", type=float, default=None)
    ap.add_argument("--wav", help="write rendered audio to this path")
    ap.add_argument("--sr", type=float, default=48000.0)
    ap.add_argument("--max-frames", type=int, default=128)
    ap.add_argument("--list-params", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    inst = Instrument(args.instrument, sample_rate=args.sr,
                      max_frames=args.max_frames, verbose=args.verbose)
    if args.list_params:
        for p in inst.manifest["params"]:
            if p["name"].startswith("__"):
                continue
            unit = p.get("unit", "")
            print(f"  {p['name']:<16} default={p.get('default', 0)} "
                  f"range=[{p.get('min')}, {p.get('max')}] {unit}")
        return

    params = {}
    for s in args.sets:
        k, v = s.split("=", 1)
        params[k] = float(v)
    ramps = dict(parse_ramp(r) for r in args.ramp)
    retrig = [float(t) for t in args.retrig.split(",") if t]

    y, _ = inst.render(args.seconds, pitch=args.pitch, vel=args.vel,
                       params=params, ramps=ramps or None,
                       retrig=retrig or None, gate_off=args.gate_off)
    mono = y if y.ndim == 1 else y[:, 0]
    report(os.path.basename(args.instrument.rstrip("/")), mono, args.sr)
    if args.wav:
        write_wav(args.wav, y, args.sr)
        print(f"wrote {args.wav}")


if __name__ == "__main__":
    main()
