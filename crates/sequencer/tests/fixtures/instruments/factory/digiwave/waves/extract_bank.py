#!/usr/bin/env python3
"""Generate the Digiwave wavetable bank: 8 banks x 64 single-cycle waves x
512 samples (shape [512, 512], wave-major: index = wave * 512 + sample,
wave index = bank * 64 + position), from the AKWF-FREE collection
(Adventure Kid Waveforms, CC0 / public domain):

  https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE

Usage:
    python3 extract_bank.py [path-to-AKWF-FREE-clone]

Only this script, bank.json, bank-manifest.txt and README.md are checked in;
the AKWF-FREE clone (WAV files) is NOT part of the repo tree.

Bank 0 ("MnM SFX 1") is the DPRO-DDRW v1 user bank and is copied VERBATIM
from ../../../monomachine/dpro/monomachine-dpro-ddrw-v1/waves/user-bank.json
so it sounds exactly as it always has. Provenance verified 2026-09-02: it is
AKWF's generic bank `AKWF/AKWF_0001` (the `AKWF--MonoMachine-SFX-60+` tree
is the same collection re-cut into 64-file folders for the Monomachine's
sampler card, byte-identical files), FFT-resampled 600 -> 512, peak-
normalized to 0.95, DC kept (correlation 1.000 per wave).

Every other bank uses the same pipeline as bank 0 so the banks sit together:
  1. load the native 600-sample / 16-bit / 44.1k single-cycle WAV,
  2. FFT resample to 512 (rfft -> truncate/zero-pad -> irfft; exact for
     periodic single cycles),
  3. peak-normalize to 0.95 (no DC removal, matching bank 0).

"folder" banks take a whole 64-file folder in file order.
Category banks pick 64 waves from a larger AKWF folder spread evenly across
the folder's spectral-centroid range and ordered dark -> bright, so a `wave`
sweep is a brightness sweep. The exact files chosen are written to
bank-manifest.txt; that manifest is the reproducibility record.
"""
import glob
import json
import os
import sys
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_AKWF_ROOT = os.path.expanduser("~/code/akwf-free")
DDRW_BANK = os.path.normpath(os.path.join(
    HERE, "..", "..", "..", "monomachine", "dpro",
    "monomachine-dpro-ddrw-v1", "waves", "user-bank.json"))

N = 512
WAVES_PER_BANK = 64
PEAK = 0.95

SOURCE = (
    "AKWF-FREE (Adventure Kid Waveforms), public domain / CC0 1.0 Universal -- "
    "https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE"
)

# (display name, kind, folder(s) relative to the AKWF-FREE clone)
#   kind "verbatim": bank 0, copied from the ddrw-v1 user bank
#   kind "folder":   all 64 files of the folder, in name order
#   kind "spread":   64 of the folders' files, spread by spectral centroid
# Lineup leans Monomachine: digital, gritty, aliased material.
BANKS = [
    ("DDRW", "verbatim", ["AKWF--MonoMachine-SFX-60+/AKWF_0001"]),
    ("AKWF 2", "folder", ["AKWF--MonoMachine-SFX-60+/AKWF_0002"]),
    ("Chip", "spread", ["AKWF/AKWF_oscchip"]),
    ("Video Game", "spread", ["AKWF/AKWF_vgame"]),
    ("Bit Reduced", "spread", ["AKWF/AKWF_bitreduced"]),
    ("Raw", "spread", ["AKWF/AKWF_raw", "AKWF/AKWF_snippets"]),
    ("Grain", "spread", ["AKWF/AKWF_distorted", "AKWF/AKWF_granular"]),
    ("FM", "spread", ["AKWF/AKWF_fmsynth"]),
]


def load_wav(path):
    with wave.open(path) as w:
        n, ch, sw = w.getnframes(), w.getnchannels(), w.getsampwidth()
        assert sw == 2, f"{path}: expected 16-bit"
        a = np.frombuffer(w.readframes(n), dtype=np.int16).astype(np.float64)
    if ch > 1:
        a = a.reshape(-1, ch).mean(axis=1)
    return a / 32768.0


def resample(a, n=N):
    X = np.fft.rfft(a)
    Y = np.zeros(n // 2 + 1, dtype=complex)
    k = min(len(X), len(Y))
    Y[:k] = X[:k]
    return np.fft.irfft(Y, n) * n / len(a)


def process(path):
    y = resample(load_wav(path))
    peak = np.abs(y).max()
    if peak > 0:
        y = y * (PEAK / peak)
    return np.round(y, 5)


def centroid(y):
    mag = np.abs(np.fft.rfft(y))[1:]
    k = np.arange(1, len(mag) + 1)
    return float((mag * k).sum() / (mag.sum() + 1e-12))


def pick_spread(files):
    ys = [process(f) for f in files]
    order = sorted(range(len(files)), key=lambda i: centroid(ys[i]))
    idx = np.linspace(0, len(order) - 1, WAVES_PER_BANK).round().astype(int)
    chosen = [order[i] for i in idx]
    return [files[i] for i in chosen], [ys[i] for i in chosen]


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_AKWF_ROOT
    data, manifest, names = [], [], []
    for name, kind, folders in BANKS:
        names.append(name)
        folder = folders[0]
        if kind == "verbatim":
            bank = json.load(open(DDRW_BANK))
            assert len(bank) == WAVES_PER_BANK * N
            data.extend(bank)
            manifest.extend(
                f"{name}\t{folder}/AKWF_{i + 1:04d}.wav\t(verbatim from ddrw-v1 user-bank.json)"
                for i in range(WAVES_PER_BANK))
            continue
        files = sorted(
            f for folder in folders
            for f in glob.glob(os.path.join(root, folder, "*.wav")))
        if not files:
            sys.exit(f"no WAVs under {folders}")
        if kind == "folder":
            assert len(files) == WAVES_PER_BANK, f"{folder}: {len(files)} files"
            ys = [process(f) for f in files]
        else:
            files, ys = pick_spread(files)
        for f, y in zip(files, ys):
            data.extend(float(v) for v in y)
            manifest.append(f"{name}\t{os.path.relpath(f, root)}")
    assert len(data) == len(BANKS) * WAVES_PER_BANK * N
    out = {
        "shape": [N, len(BANKS) * WAVES_PER_BANK],
        "kind": "wavetable-bank",
        "layout": "wave-major: index = wave * 512 + sample",
        "source": SOURCE,
        "sets": names,
        "waves_per_set": WAVES_PER_BANK,
        "data": data,
    }
    with open(os.path.join(HERE, "bank.json"), "w") as fh:
        json.dump(out, fh, separators=(",", ":"))
    with open(os.path.join(HERE, "bank-manifest.txt"), "w") as fh:
        fh.write("# bank\tsource file (relative to AKWF-FREE clone)\n")
        fh.write("\n".join(manifest) + "\n")
    print(f"wrote bank.json: {len(BANKS)} banks x {WAVES_PER_BANK} waves")


if __name__ == "__main__":
    main()
