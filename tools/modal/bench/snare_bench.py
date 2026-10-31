#!/usr/bin/env python3
"""Score a snare instrument against the factory Membrane Snare.

Renders the reference (content/instruments/Drums/Membrane Snare, default
patch) and a candidate instrument folder at the same velocities, and compares
what the modal-snare work found matters (see the modal-snare-instrument notes):

  envelope   level per band (low <300 Hz, mid 300-2k, buzz 2-8k, air >8k)
             in 20 ms bins over the first 300 ms, in dB under each render's
             own peak, so the shapes are compared, not the loudness
  rattle     the wires' contribution, full minus wires-off, in 20 ms bins
             (the reference's wires-off is its contact mic at 0, the
             candidate's whatever --wires-off sets)
  dynamics   how buzz-band energy grows with velocity (the reference's is
             super-linear: a harder hit buzzes disproportionately more)
  tone       the spectrum of the first 150 ms in third-octave bands from
             100 Hz to 16 kHz, in dB under its loudest band: the timbre
             the band envelopes are too coarse to see
  pitch      the strongest low peaks of the first 300 ms
  decay      low-band T60 and the time the buzz band falls 40 dB

Usage (from the repo root; needs the pinned compiler, see AGENTS.md):
  ESEQ_DGENLISP_TOOL=$PWD/crates/sequencer/tools/DGenLisp-macos-arm64 \\
  ESEQ_DGEN_TOOLCHAIN_ROOT=$PWD/crates/sequencer/tools/dgen-toolchain \\
  python3 tools/modal/bench/snare_bench.py CANDIDATE_DIR \\
      [--pitch HZ] [--set group.param=value ...] [--wires-off group.param=value ...] \\
      [--out DIR]

With no CANDIDATE_DIR it reports the reference alone.
"""

import argparse
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, os.path.join(REPO, "tools", "audition"))
from audition import Instrument  # noqa: E402

REFERENCE = os.path.join(REPO, "content", "instruments", "Drums", "Membrane Snare")
SR = 48000
SECONDS = 1.0
BIN = 0.02
WINDOW = 0.3
VELOCITIES = (0.25, 0.5, 1.0)
BANDS = (("low", 0, 300), ("mid", 300, 2000), ("buzz", 2000, 8000), ("air", 8000, 24000))


def mono(y):
    """A render's audio as one channel (a stereo one's two averaged)."""
    y = np.asarray(y[0] if isinstance(y, (tuple, list)) else y, dtype=float)
    return y.mean(axis=1) if y.ndim == 2 else y


def band_signal(y, lo, hi):
    spectrum = np.fft.rfft(y)
    f = np.fft.rfftfreq(len(y), 1 / SR)
    spectrum[(f < lo) | (f >= hi)] = 0
    return np.fft.irfft(spectrum, len(y))


def bins_db(y, ref_peak):
    n = int(BIN * SR)
    count = int(WINDOW / BIN)
    out = []
    for k in range(count):
        seg = y[k * n:(k + 1) * n]
        rms = np.sqrt(np.mean(seg ** 2)) if len(seg) else 0.0
        out.append(20 * np.log10(rms / ref_peak + 1e-9))
    return np.maximum(np.array(out), -90)


def profile(y):
    """Per-band 20 ms levels in dB under the render's own peak RMS bin."""
    n = int(BIN * SR)
    peak = max(np.sqrt(np.mean(y[k:k + n] ** 2)) for k in range(0, int(WINDOW * SR), n)) + 1e-12
    return {name: bins_db(band_signal(y, lo, hi), peak) for name, lo, hi in BANDS}, peak


def t_fall(y, lo, hi, drop_db):
    """Seconds until a band's 20 ms level is drop_db under its max."""
    b = band_signal(y, lo, hi)
    n = int(BIN * SR)
    levels = [np.sqrt(np.mean(b[k:k + n] ** 2)) + 1e-12 for k in range(0, len(b) - n, n)]
    db = 20 * np.log10(np.array(levels) / max(levels))
    top = int(np.argmax(db))
    below = np.where(db[top:] < -drop_db)[0]
    return (top + below[0]) * BIN if len(below) else float("nan")


def low_peaks(y, count=3):
    seg = y[:int(WINDOW * SR)]
    spectrum = np.abs(np.fft.rfft(seg * np.hanning(len(seg)), 8 * len(seg)))
    f = np.fft.rfftfreq(8 * len(seg), 1 / SR)
    keep = (f > 40) & (f < 600)
    spectrum, f = spectrum[keep], f[keep]
    peaks = [k for k in range(1, len(spectrum) - 1) if spectrum[k] > spectrum[k - 1] and spectrum[k] >= spectrum[k + 1]]
    peaks.sort(key=lambda k: -spectrum[k])
    return sorted(int(round(f[k])) for k in peaks[:count])


THIRDS = 100 * 2 ** (np.arange(0, 22) / 3)  # 100 Hz .. 16 kHz


def tone(y, seconds=0.15):
    """Third-octave band levels of the first seconds, dB under the loudest."""
    seg = y[:int(seconds * SR)]
    power = np.abs(np.fft.rfft(seg * np.hanning(len(seg)))) ** 2
    f = np.fft.rfftfreq(len(seg), 1 / SR)
    bands = np.array([np.sum(power[(f >= lo / 2 ** (1 / 6)) & (f < lo * 2 ** (1 / 6))]) for lo in THIRDS]) + 1e-30
    db = 10 * np.log10(bands / bands.max())
    return np.maximum(db, -60)


def band_energy(y, lo, hi, seconds=WINDOW):
    b = band_signal(y[:int(seconds * SR)], lo, hi)
    return float(np.sum(b ** 2))


def parse_sets(items):
    out = {}
    for item in items:
        name, value = item.split("=", 1)
        out[name] = float(value)
    return out


class Subject:
    def __init__(self, path, pitch, sets, wires_off):
        self.name = os.path.basename(os.path.normpath(path))
        self.inst = Instrument(path, sample_rate=SR)
        self.pitch = pitch
        self.sets = sets
        self.wires_off = wires_off

    def render(self, vel, extra=None):
        params = dict(self.sets)
        params.update(extra or {})
        return mono(self.inst.render(SECONDS, pitch=self.pitch, vel=vel, params=params))

    def measure(self):
        full = {v: self.render(v) for v in VELOCITIES}
        dry = self.render(1.0, self.wires_off) if self.wires_off else None
        hit = full[1.0]
        prof, peak = profile(hit)
        rattle = bins_db(hit - dry, peak) if dry is not None else None
        buzz = {v: band_energy(full[v], 2000, 8000) for v in VELOCITIES}
        total = {v: band_energy(full[v], 0, 24000) for v in VELOCITIES}
        return {
            "full": full, "dry": dry, "profile": prof, "rattle": rattle,
            "buzz_growth_db": 10 * np.log10(buzz[1.0] / max(buzz[0.25], 1e-30)),
            "total_growth_db": 10 * np.log10(total[1.0] / max(total[0.25], 1e-30)),
            "peaks": low_peaks(hit),
            "tone": tone(hit),
            "low_t60": t_fall(hit, 0, 300, 60),
            "buzz_t40": t_fall(hit, 2000, 8000, 40),
            "nan": bool(np.any(~np.isfinite(hit))),
            "peak": float(np.max(np.abs(hit))),
        }


def fmt_bins(row, every=2):
    return " ".join(f"{v:5.0f}" for v in row[::every])


def report(label, m):
    print(f"== {label}")
    print(f"   peak {m['peak']:.3f}  nan {m['nan']}  low peaks Hz {m['peaks']}  "
          f"low T60 {m['low_t60']:.2f}s  buzz -40dB at {m['buzz_t40']:.2f}s")
    print(f"   velocity 0.25->1: total +{m['total_growth_db']:.1f} dB, buzz +{m['buzz_growth_db']:.1f} dB")
    print("   dB under peak, 40 ms steps 0..300 ms:")
    for name, _, _ in BANDS:
        print(f"     {name:5s} {fmt_bins(m['profile'][name])}")
    if m["rattle"] is not None:
        print(f"     wires {fmt_bins(m['rattle'])}")
    print("   tone, dB per third octave 100 Hz..16 kHz (every other):")
    print(f"           {fmt_bins(m['tone'])}")


def score(ref, cand):
    """Mean |dB| difference per band over 0..300 ms (bins above -70 in either)."""
    parts = {}
    for name, _, _ in BANDS:
        a, b = ref["profile"][name], cand["profile"][name]
        keep = (a > -70) | (b > -70)
        parts[name] = float(np.mean(np.abs(a[keep] - b[keep]))) if np.any(keep) else 0.0
    if ref["rattle"] is not None and cand["rattle"] is not None:
        a, b = ref["rattle"], cand["rattle"]
        keep = (a > -70) | (b > -70)
        parts["wires"] = float(np.mean(np.abs(a[keep] - b[keep]))) if np.any(keep) else 0.0
    parts["dynamics"] = abs(ref["buzz_growth_db"] - cand["buzz_growth_db"])
    parts["tone"] = float(np.mean(np.abs(ref["tone"] - cand["tone"])))
    return parts


def write_wav(path, y):
    import wave
    y = y / (np.max(np.abs(y)) + 1e-12) * 0.9
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes((y * 32767).astype(np.int16).tobytes())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("candidate", nargs="?")
    ap.add_argument("--pitch", type=float, default=440.0, help="candidate's pitch input, Hz")
    ap.add_argument("--set", dest="sets", action="append", default=[])
    ap.add_argument("--wires-off", action="append", default=[])
    ap.add_argument("--out", help="write reference and candidate WAVs here")
    args = ap.parse_args()

    ref = Subject(REFERENCE, 440.0, {}, {"snares": 0.0}).measure()
    report("Membrane Snare (reference)", ref)
    cand = None
    if args.candidate:
        cand = Subject(args.candidate, args.pitch, parse_sets(args.sets), parse_sets(args.wires_off)).measure()
        report(os.path.basename(os.path.normpath(args.candidate)), cand)
        parts = score(ref, cand)
        print("== score (mean |dB| per band, lower is closer): "
              + "  ".join(f"{k} {v:.1f}" for k, v in parts.items())
              + f"  | total {sum(parts.values()):.1f}")
    if args.out:
        os.makedirs(args.out, exist_ok=True)
        for v in VELOCITIES:
            write_wav(os.path.join(args.out, f"reference-v{v}.wav"), ref["full"][v])
            if cand:
                write_wav(os.path.join(args.out, f"candidate-v{v}.wav"), cand["full"][v])
        print(f"wrote WAVs to {args.out}")


if __name__ == "__main__":
    main()
