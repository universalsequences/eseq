#!/usr/bin/env python3
"""The assembled instrument at a host rate (48 kHz by default) against the
32 kHz C++ Part, resampled. Compares the module's L/R outputs (after the
codec stage) with the instrument's left/right at level 0 dB: RMS, the
waveform error after resampling, and the error of the 1/3-octave spectrum.

usage: srcmp.py INSTRUMENT_DIR [sr]
"""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

PMAP = {"blow_meta": "flow", "strike_meta": "mallet"}


def resample(y, sr_in, sr_out):
    n = len(y)
    m = int(round(n * sr_out / sr_in))
    Y = np.fft.rfft(y)
    Z = np.zeros(m // 2 + 1, dtype=complex)
    k = min(len(Y), len(Z))
    Z[:k] = Y[:k]
    return np.fft.irfft(Z, m) * (m / n)


def soft(x):
    return x * (27 + x * x) / (27 + 9 * x * x)


def bands(y, sr):
    s = np.abs(np.fft.rfft(y * np.hanning(len(y)))) ** 2
    f = np.fft.rfftfreq(len(y), 1 / sr)
    edges = 50 * 2 ** (np.arange(0, 28) / 3)
    out = []
    for a, b in zip(edges[:-1], edges[1:]):
        if b > 14000:
            break
        out.append(np.sum(s[(f >= a) & (f < b)]) + 1e-20)
    return 10 * np.log10(np.array(out))


CASES = [
    ("mallet", dict()),
    ("mallet bright", dict(brightness=0.9, damping=0.6)),
    ("mallet geo .7", dict(geometry=0.7)),
    ("sample", dict(strike_meta=0.15)),
    ("plectrum", dict(strike_meta=0.72)),
    ("note 84", dict(note=84)),
    ("string", dict(model=1)),
    ("string geo .6", dict(model=1, geometry=0.6, note=40)),
    ("strings", dict(model=2, geometry=0.4)),
    ("ominous", dict(easter=1, contour=0.8, blow=0.8, strike=0.5, gate=0.5)),
    ("reverb", dict(space=0.9)),
]


def main():
    inst_dir = sys.argv[1]
    sr = float(sys.argv[2]) if len(sys.argv) > 2 else 48000.0
    inst = elh.A.Instrument(inst_dir, sample_rate=sr)
    t = elh.Test.__new__(elh.Test)
    t.dir = os.path.join(elh.WORK, "srcmp")
    os.makedirs(t.dir, exist_ok=True)
    for label, kw in CASES:
        kw = dict(dict(space=0, seconds=1.0, gate=0.5), **kw)
        r = t.ref(**kw)
        ref_l = soft(r[:, 1] * 0.5)
        ref_r = soft(r[:, 0] * 0.5)
        params = {"level_db": 0.0, "vel_amt": 0.0, "freeze": 0.0}
        for k, v in kw.items():
            if k in ("seconds", "gate", "note", "easter", "det"):
                continue
            params[PMAP.get(k, k)] = float(v)
        if kw.get("easter"):
            params["model"] = 3.0
        hz = 440 * 2 ** ((kw.get("note", 48) - 69) / 12)
        y, _ = inst.render(kw["seconds"], pitch=hz, params=params, gate_off=kw["gate"])
        y = np.asarray(y)
        parts = []
        for nm, a, b in (("L", ref_l, y[:, 0]), ("R", ref_r, y[:, 1])):
            a = resample(a.astype(float), 32000, sr)
            n = min(len(a), len(b))
            a, b = a[:n], b[:n].astype(float)
            ra = np.sqrt(np.mean(a ** 2)); rb = np.sqrt(np.mean(b ** 2))
            w = slice(0, int(0.1 * sr))
            e0 = np.sqrt(np.mean((a[w] - b[w]) ** 2)) / max(np.sqrt(np.mean(a[w] ** 2)), 1e-12)
            bd = np.max(np.abs(bands(a, sr) - bands(b, sr))[:22])
            parts.append(f"{nm}: rms {ra:.4f}/{rb:.4f} ({20*np.log10(max(rb,1e-12)/max(ra,1e-12)):+.2f} dB) wave100 {e0:.3f} band max {bd:.1f} dB")
        print(f"{label:14s} " + "  ".join(parts), flush=True)


if __name__ == "__main__":
    main()
