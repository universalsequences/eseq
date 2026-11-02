"""Training sets for eseq.modal.train experiments: samples from library packs and tag queries,
each cut 2 ms before its FIRST hit (not its loudest), 44.1 kHz 16-bit stereo, a 0.5 ms fade-in.

build(spec, out, library) writes out/<name>.wav + out/manifest.json and out/crop.json.

spec (experiment.json "data"):
  packs       pack names (sample-packs.json)
  tags        tag sets; a sample carrying every tag of one set is in (case-insensitive)
  keep-crops  a dataset dir (manifest.json with hashes): its samples keep their exact files and names
  only-kept   true: only the keep-crops samples the packs / tags select (a fixed set to compare runs on)
  crop        "first-hit" or "none"
Names: a kept sample keeps its name; a pack member B<k> by pack position; a tagged sample a short
code from its title. The first-hit detector is the break-snares one (spectral-flux onsets, refined
on the high-passed 1 ms envelope against the background floor); a one-shot whose cut-away part
reaches half the peak starts on its first loud sample instead (the flux picks the strongest onset).
"""
import json
import os
import re
import shutil
import struct
import sys
from math import gcd

import numpy as np
from scipy import signal
from scipy.io import wavfile

SR = 44100
HOP = 128
NFFT = 1024
FIRST_REL = 0.25
PRE = 0.002
FADE = 0.0005
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "../../sample-packs"))
import export_pack  # noqa: E402


def raw_wav(path):
    """RIFF reader for headers scipy rejects (a wrong nAvgBytesPerSec): PCM 8/16/24/32, float 32/64."""
    b = open(path, "rb").read()
    i, fmt = 12, None
    while i + 8 <= len(b):
        cid, size = b[i:i + 4], struct.unpack("<I", b[i + 4:i + 8])[0]
        body = b[i + 8:i + 8 + size]
        if cid == b"fmt ":
            tag, ch, sr, _, _, bits = struct.unpack("<HHIIHH", body[:16])
            if tag == 0xFFFE:
                tag = struct.unpack("<H", body[24:26])[0]
            fmt = (tag, ch, sr, bits)
        elif cid == b"data":
            tag, ch, sr, bits = fmt
            w = bits // 8
            body = body[:len(body) // (w * ch) * w * ch]
            if tag == 3:
                x = np.frombuffer(body, "<f4" if bits == 32 else "<f8").astype(np.float64)
            elif bits == 24:
                a = np.frombuffer(body, np.uint8).reshape(-1, 3).astype(np.int32)
                v = a[:, 0] | (a[:, 1] << 8) | (a[:, 2] << 16)
                x = np.where(v >= 1 << 23, v - (1 << 24), v) / float(1 << 23)
            elif bits == 8:
                x = (np.frombuffer(body, np.uint8).astype(np.float64) - 128) / 128
            else:
                x = np.frombuffer(body, {16: "<i2", 32: "<i4"}[bits]).astype(np.float64) / float(1 << (bits - 1))
            return sr, x.reshape(-1, ch)
        i += 8 + size + (size & 1)
    raise ValueError(f"{path}: no data chunk")


def read(path):
    """(native rate, [n, 2] float at 44.1 kHz)."""
    try:
        sr, x = wavfile.read(path)
        if x.dtype == np.uint8:
            x = (x.astype(np.float64) - 128) / 128
        else:
            x = x.astype(np.float64) / (float(-np.iinfo(x.dtype).min) if x.dtype.kind == "i" else 1.0)
    except ValueError:
        sr, x = raw_wav(path)
    if x.ndim == 1:
        x = np.stack([x, x], axis=1)
    if x.shape[1] == 1:
        x = np.concatenate([x, x], axis=1)
    if sr != SR:
        g = gcd(sr, SR)
        x = signal.resample_poly(x, SR // g, sr // g, axis=0)
    return sr, x


def onsets(m):
    f, t, Z = signal.stft(m, SR, nperseg=NFFT, noverlap=NFFT - HOP, boundary=None, padded=True)
    S = np.abs(Z)
    L = np.log1p(100 * S)
    flux = np.concatenate([[0], np.maximum(np.diff(L, axis=1), 0).sum(axis=0)])
    flux = flux / (flux.max() + 1e-12)
    peaks, _ = signal.find_peaks(flux, height=signal.medfilt(flux, 15) + 0.12, distance=int(0.035 * SR / HOP))
    return [dict(t=float(t[p]), strength=float(flux[p])) for p in peaks], t, S


def first_hit(m):
    ons, t, S = onsets(m)
    if not ons:
        return 0.0, ons
    strongest = max(o["strength"] for o in ons)
    env = 20 * np.log10(np.sqrt((S ** 2).sum(axis=0)) + 1e-12)
    for o in ons:
        i = int(np.searchsorted(t, o["t"]))
        if o["strength"] >= FIRST_REL * strongest and env[i:i + int(0.03 * SR / HOP)].max() >= env.max() - 24:
            return o["t"], ons
    return ons[0]["t"], ons


def refine(m, t_coarse):
    """Sample-accurate attack start: where the 1 ms envelope of the 300 Hz high-passed signal crosses
    15% of the way from the background floor to the local peak, in [t - 25 ms, t + 10 ms]."""
    hp = signal.filtfilt(*signal.butter(2, 300, btype="high", fs=SR), m)
    env = signal.filtfilt(*signal.butter(2, 1000, fs=SR), np.abs(hp))
    a = max(int((t_coarse - 0.025) * SR), 0)
    b = min(int((t_coarse + 0.03) * SR), len(env))
    c = min(int((t_coarse + 0.01) * SR), len(env))
    peak = env[a:b].max()
    floor = float(np.percentile(env[:max(int((t_coarse - 0.005) * SR), 1)], 50)) if t_coarse > 0.008 else 0.0
    floor = min(floor, 0.5 * peak)
    above = np.where(env[a:c] >= floor + 0.15 * (peak - floor))[0]
    return (a + above[0]) / SR if len(above) else t_coarse


def cut_point(x):
    m = x.mean(axis=1)
    t_hit, _ = first_hit(m)
    i0 = max(int((refine(m, t_hit) - PRE) * SR), 0)
    a = np.abs(m)
    if i0 and a[:i0].max() >= 0.5 * a.max():
        i0 = max(int(np.argmax(a > 0.1 * a.max())) - int(PRE * SR), 0)
    return i0


def code(title, taken):
    stem = re.sub(r"\.(wav|aif+|flac|mp3)$", "", title or "", flags=re.I)
    stem = re.sub(r"[-_ ]?(snr|snare)$", "", stem, flags=re.I)
    c = re.sub(r"[^A-Za-z0-9]", "", stem).upper()[:10] or "S"
    name, n = c, 2
    while name in taken:
        name, n = f"{c}{n}", n + 1
    taken.add(name)
    return name


def sources(spec, library):
    """[(hash, title, region-or-None, origin)] in order: packs first, then tag matches."""
    titles, regions = export_pack.load_journal(library)
    out, seen = [], set()
    packs = {p["name"].lower(): p for p in export_pack.load_packs(library)}
    for name in spec.get("packs") or []:
        pack = packs.get(name.lower())
        if pack is None:
            raise SystemExit(f"no pack named {name!r}; packs: {', '.join(p['name'] for p in packs.values())}")
        for h in pack["samples"]:
            if h not in seen:
                seen.add(h)
                out.append((h, titles.get(h), regions.get(h), "pack:" + name))
    tags = {}
    for line in open(os.path.join(library, "samples.jsonl")):
        if line.strip():
            f = json.loads(line)
            if f.get("kind") == "sample":
                tags[f["hash"]] = {t.lower() for t in f.get("tags", [])}
    for tset in spec.get("tags") or []:
        want = {t.lower() for t in tset}
        for h in sorted(tags, key=lambda h: (titles.get(h) or "").lower()):
            if want <= tags[h] and h not in seen:
                seen.add(h)
                out.append((h, titles.get(h), regions.get(h), "tags:" + "+".join(tset)))
    return out


def build(spec, out, library, root):
    os.makedirs(out, exist_ok=True)
    keep = {}
    if spec.get("keep-crops"):
        kd = os.path.join(root, spec["keep-crops"])
        for m in json.load(open(os.path.join(kd, "manifest.json"))):
            keep[m["hash"]] = (kd, m)
    taken = {m["name"] for _, m in keep.values()}
    nb = max([int(n[1:]) for n in taken if re.fullmatch(r"B\d+", n)] + [0])
    man, crops = [], []
    for h, title, region, origin in sources(spec, library):
        if spec.get("only-kept") and h not in keep:
            continue
        if h in keep:
            kd, m = keep[h]
            shutil.copyfile(os.path.join(kd, m["name"] + ".wav"), os.path.join(out, m["name"] + ".wav"))
            man.append(dict(m, origin=origin))
            crops.append(dict(name=m["name"], kept=True))
            continue
        if origin.startswith("pack:"):
            nb += 1
            name = f"B{nb:02d}"
            taken.add(name)
        else:
            name = code(title, taken)
        src = os.path.join(library, "samples", h + ".wav")
        _, x = read(src)
        i0 = cut_point(x) if spec.get("crop", "first-hit") == "first-hit" else 0
        y = x[i0:].copy()
        nf = int(FADE * SR)
        y[:nf] *= np.linspace(0, 1, nf)[:, None]
        pcm = np.clip(np.round(y * 32767), -32768, 32767).astype(np.int16)
        wavfile.write(os.path.join(out, name + ".wav"), SR, pcm)
        man.append(dict(name=name, title=title, hash=h, sr=SR, ch=2, bits=16, frames=len(pcm), origin=origin))
        crops.append(dict(name=name, cut_ms=round(1000 * i0 / SR, 1)))
    json.dump(man, open(os.path.join(out, "manifest.json"), "w"), indent=1)
    json.dump(crops, open(os.path.join(out, "crop.json"), "w"), indent=1)
    return man
