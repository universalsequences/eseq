"""Starting rows for eseq.modal.train parts, from the layout (layout.json) and the target audio.

Rows are {sample: {segment: [values]}} in natural units (what row-layout squashes into its range).

neutral(layout)          every segment at a neutral value: level fields at their lower bound (the part
                         is silent), frequencies log-spaced over their range, log fields at the
                         geometric middle, lin fields at the middle, raw at 0
from_snare1(...)         copy dgen `drum-rows --layout snare1` analysis rows onto the parts it covers
                         (the head modes, strikes, chain, wires, hiss, crackle, events), by field
seed_free_modes(...)     free (not note-tracked) modes: the strongest lines the current render misses
seed_template(...)       a template's level: ~10% of the free modes' energy (or a small floor)
seed_wash(...)           a wash: 0.1 x the bank's slow level in the bands >= 2 kHz
seed_restrike(...)       a second stroke at the largest > 4 kHz envelope rise after 50 ms (>= 6 dB)
"""
import numpy as np
from scipy import signal
from scipy.io import wavfile

SR = 44100
BAND_FC = [48.23634, 70.14603, 102.0074, 148.3408, 215.7195, 313.7026, 456.1912, 663.4002, 964.7268, 1402.921,
           2040.149, 2966.816, 4314.39, 6274.052, 9123.823, 13268]
SNARE1_KINDS = {"strikes", "shaper", "clipper", "record-lp", "timing", "bands", "hiss", "crackle", "events"}


def rd(p):
    """Mono float at the file's rate (any PCM width, float, or a header scipy rejects)."""
    from dataset import read
    _, x = read(p)
    return x.mean(1)


def segs(layout, part=None, kind=None):
    return [s for s in layout["segments"] if (part is None or s["part"] == part) and (kind is None or s["kind"] == kind)]


def seg_of(layout, part, field):
    for s in layout["segments"]:
        if s["part"] == part and s["field"] == field:
            return s["name"]
    return None


def neutral_value(s):
    n = s["count"]
    if s["scale"] == "raw":
        return [0.0] * n
    lo, hi = s["lo"], s["hi"]
    if s["role"] == "level":
        return [float(lo)] * n
    if s["role"] == "freq" and s["scale"] == "log":
        return [float(v) for v in np.geomspace(max(lo, 1e-9) * 1.05, hi / 1.05, n)] if n > 1 else [float(np.sqrt(lo * hi))]
    if s["scale"] == "log":
        return [float(np.sqrt(lo * hi))] * n
    return [float(0.5 * (lo + hi))] * n


def neutral(layout):
    return {s["name"]: neutral_value(s) for s in layout["segments"]}


def head_part(layout):
    for p in layout["parts"]:
        if p["kind"] == "modes" and p["follow-note"]:
            return p["id"]
    return None


def from_snare1(row, snare1, layout):
    """Fields of the parts a snare1 analysis covers; returns the parts it filled."""
    filled = set()
    head = head_part(layout)
    for p in layout["parts"]:
        if p["kind"] in SNARE1_KINDS or p["id"] == head:
            for s in segs(layout, part=p["id"]):
                v = snare1.get(s["field"])
                if v is not None and len(v) == s["count"]:
                    row[s["name"]] = [float(x) for x in v]
                    filled.add(p["id"])
    return filled


def by_name(row, src, layout):
    """Every segment the analysis rows have under its own name (kick-rows v3 names = the kick voice's);
    returns the parts whose segments were all filled."""
    filled, missed = set(), set()
    for s in layout["segments"]:
        v = src.get(s["name"])
        if v is not None and len(v) == s["count"]:
            row[s["name"]] = [float(x) for x in v]
            filled.add(s["part"])
        else:
            missed.add(s["part"])
    return filled - missed


def spectra(x):
    """The target's spectra every line needs: Welch after 40 ms, and a 4096 / 256 STFT power."""
    s0 = int(0.04 * SR)
    fw, Pw = signal.welch(x[s0:], SR, nperseg=min(4096, len(x) - s0))
    f, tt, Z = signal.stft(x, SR, nperseg=4096, noverlap=4096 - 256)
    return dict(fw=fw, Pw=Pw, f=f, tt=tt, P=np.abs(Z) ** 2, n=len(x) - s0)


def missing_lines(t, y, lo, hi, k, sp):
    s = int(0.04 * SR)
    f, Pt = sp["fw"], sp["Pw"]
    _, Py = signal.welch(y[s:len(t)], SR, nperseg=min(4096, len(t) - s))
    band = (f > 200) & (f < 16000)
    g = Pt[band].sum() / max(Py[band].sum(), 1e-30)
    Lt = 10 * np.log10(Pt + 1e-30)
    Ly = 10 * np.log10(g * Py + 1e-30)
    pk, _ = signal.find_peaks(Lt - signal.medfilt(Lt, 41), height=8, distance=4)
    out = [(f[p], Lt[p]) for p in pk if lo < f[p] < hi and Lt[p] - Ly[p] > 6 and Lt[p] - Lt.max() > -50]
    out.sort(key=lambda r: -r[1])
    return [fr for fr, _ in out[:k]]


def line_track(sp, fr):
    f, tt, P = sp["f"], sp["tt"], sp["P"]
    k = int(np.argmin(np.abs(f - fr)))
    line = P[max(k - 2, 0):k + 3].sum(axis=0)
    side = np.concatenate([P[max(k - 20, 0):max(k - 6, 0)], P[k + 6:k + 20]])
    lp = np.maximum(line - (np.median(side, axis=0) * 5 if len(side) else 0), 1e-30)
    fw, Pw = sp["fw"], sp["Pw"]
    kw = int(np.argmin(np.abs(fw - fr)))
    pw = Pw[max(kw - 2, 0):kw + 3].sum() - 5 * np.median(np.concatenate([Pw[max(kw - 20, 0):max(kw - 6, 0)], Pw[kw + 6:kw + 20]]))
    energy = float(max(pw, 0) * (fw[1] - fw[0]) * sp["n"])
    i0 = int(np.argmax(lp[: max(int(0.1 * SR / 256), 2)]))
    db = 10 * np.log10(lp[i0:] / lp[i0])
    ok = np.where(db > -30)[0]
    if len(ok) >= 4:
        slope = np.polyfit(tt[i0:][ok] - tt[i0], db[ok], 1)[0]
        tau = float(np.clip(-8.686 / min(slope, -1e-3), 0.006, 5.5))
    else:
        tau = 0.2
    return energy, tau


def gain_of(row, layout):
    g = seg_of(layout, next(p["id"] for p in layout["parts"] if p["kind"] == "timing"), "gain")
    return row[g][0] if g else 1.0


def seed_free_modes(row, part, layout, t, render, rng):
    sf, sa, sd, sp = (seg_of(layout, part, k) for k in ("f", "amp", "dec", "ph"))
    fseg = next(s for s in layout["segments"] if s["name"] == sf)
    k, lo, hi = fseg["count"], fseg["lo"], fseg["hi"]
    y = rd(render)
    spec = spectra(t)
    lev = (y[:len(t)] ** 2).sum() / max((t ** 2).sum(), 1e-30)
    lines = missing_lines(t, y, max(lo, 1000), min(hi, 17000), k, spec)
    mf, ma, md = [], [], []
    for fr in lines:
        e, tau = line_track(spec, fr)
        a = np.sqrt(4 * e * lev / (tau * SR) / np.exp(-0.08 / tau)) / gain_of(row, layout)
        mf.append(float(fr)); ma.append(float(np.clip(a, 2e-7, 1.5))); md.append(tau)
    spare = np.geomspace(max(lo, 2000), min(hi, 16000), k)
    for j in range(len(lines), k):
        mf.append(float(spare[j])); ma.append(1.5e-7); md.append(0.1)
    row.update({sf: mf, sa: ma, sd: md, sp: [float(v) for v in rng.uniform(-np.pi, np.pi, k)]})
    return len(lines)


TEMPLATE_TAU = 0.5   # a template's typical mode decay (s), for its seeded energy


def seed_template(row, part, layout, mean_tau=TEMPLATE_TAU):
    free = [p["id"] for p in layout["parts"] if p["kind"] == "modes" and not p["follow-note"]]
    if free:
        a, d = row[seg_of(layout, free[0], "amp")], row[seg_of(layout, free[0], "dec")]
        e = sum(x * x * y / 4 for x, y in zip(a, d))
        lvl = float(np.clip(np.sqrt(0.1 * e / (mean_tau / 4)), 1e-5, 1.0))
    else:
        lvl = 1e-4
    row[seg_of(layout, part, "lvl")] = [lvl]
    row[seg_of(layout, part, "size")] = [1.0]
    row[seg_of(layout, part, "tilt")] = [0.0]
    row[seg_of(layout, part, "dec")] = [1.0]


def seed_wash(row, part, layout, bank):
    wl, wm = row[seg_of(layout, bank, "wl")], row[seg_of(layout, bank, "wm")]
    slow = [a * (1 - b) for a, b in zip(wl, wm)]
    row[seg_of(layout, part, "l")] = [float(max(0.1 * s, 1e-6)) if fc >= 2000 else 1e-6 for s, fc in zip(slow, BAND_FC)]
    row[seg_of(layout, part, "rise")] = [0.02]
    row[seg_of(layout, part, "dec")] = [0.4]


def seed_restrike(row, part, layout, x):
    hp = signal.filtfilt(*signal.butter(4, 4000, btype="high", fs=SR), x)
    e = 20 * np.log10(np.sqrt(np.maximum(signal.filtfilt(np.ones(220) / 220, [1], hp ** 2), 0)) + 1e-9)
    d = e[441:] - e[:-441]
    s = int(0.05 * SR)
    hit, t_r, rise = False, 0.4, 0.0
    if len(d) > s:
        i = s + int(np.argmax(d[s:]))
        t_r, rise = i / SR, float(d[i])
        hit = rise >= 6
    row[seg_of(layout, part, "t")] = [float(np.clip(t_r, 0.03, 0.75)) if hit else 0.4]
    row[seg_of(layout, part, "lvl")] = [0.5 if hit else 1e-4]
    return (t_r, rise) if hit else None


def scale_levels(row, part, layout, factor):
    for s in segs(layout, part=part):
        if s["role"] == "level":
            row[s["name"]] = [max(v * factor, s["lo"]) for v in row[s["name"]]]


def convert_second_stroke(row, parent, layout):
    """A part whose second stroke became absolute (template lvl2, wash l2) starts where the parent's
    relative stroke was: lvl2 = lvl x the restrike's level, l2 = l x it. Returns the parts converted."""
    rs = [p["id"] for p in layout["parts"] if p["kind"] == "restrike"]
    if not rs:
        return []
    lvl = seg_of(layout, rs[0], "lvl")
    if lvl is None or lvl not in parent:
        return []
    k = parent[lvl][0]
    done = []
    for p in layout["parts"]:
        for field, base in (("lvl2", "lvl"), ("l2", "l")):
            s2, s1 = seg_of(layout, p["id"], field), seg_of(layout, p["id"], base)
            if s2 and s1 and s2 not in parent and s1 in parent:
                row[s2] = [v * k for v in parent[s1]]
                done.append(p["id"])
    return done
