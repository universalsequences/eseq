#!/usr/bin/env python3
"""Tune the dispersive spring reverb (crates/sequencer/src/effects/spring.rs) against
a reference IR (default: spring_reverb_impulse.wav; see impulses/prepared/ for
the other spring types).

Usage:
  python3 scripts/spring_tune.py analyze [--ref F]       # measure the reference
  python3 scripts/spring_tune.py plots [--ref F] [--params F] [--tag NAME]
  python3 scripts/spring_tune.py optimize --stage A|B|C [--ref F] [--params F] [--maxiter N]

References must be 16-bit / 44.1 kHz / mono wav — scripts/prepare_spring_refs.py
converts the raw impulses/ captures.

The Rust render binary (target/release/spring_tune) is the single source of
truth for the candidate DSP; this script only analyzes and optimizes.
"""

import argparse
import json
import math
import os
import subprocess
import sys
import tempfile

import numpy as np
from scipy.optimize import minimize
from scipy.signal import stft
from scipy.special import erfc

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "release", "spring_tune")
REF_WAV = os.path.join(ROOT, "spring_reverb_impulse.wav")
OUT_DIR = os.path.join(ROOT, "tuning_out")
SR = 44100
SECONDS = 6.5

# ── Parameter space ──────────────────────────────────────────────────────────
# name -> (json path, transform). Transforms keep Nelder-Mead unconstrained:
# log for positive quantities, logit for (0,1) gains, linear otherwise.

DEFAULT_PARAMS = {
    "ap_per_loop": 80,
    "d_loop": [0.0545, 0.0754, 0.0976],
    "w_loop": [1.0, 0.8, 0.65],
    "t60": 4.5,
    "t_dc": 0.002,
    "f_peak": 4300.0,
    "f_damp": 2800.0,
    "f_shelf": 250.0,
    "g_shelf": 0.92,
    "f_hp": 80.0,
    "f_lp": 4500.0,
    "c_df": 0.55,
    "d_df1": 0.0047,
    "d_hf": 0.0023,
    "t60_hf": 0.5,
    "f_hf_damp": 4000.0,
    "g_hf": 0.15,
}

def p_get(params, name):
    if name.startswith("d_loop"):
        return params["d_loop"][int(name[-1]) - 1]
    if name.startswith("w_loop"):
        return params["w_loop"][int(name[-1]) - 1]
    return params[name]

def p_set(params, name, value):
    if name.startswith("d_loop"):
        params["d_loop"][int(name[-1]) - 1] = value
    elif name.startswith("w_loop"):
        params["w_loop"][int(name[-1]) - 1] = value
    else:
        params[name] = value

TRANSFORMS = {
    # log-space params
    **{n: "log" for n in [
        "d_loop1", "d_loop2", "d_loop3", "t60", "t_dc", "f_peak", "f_damp", "f_shelf",
        "f_hp", "f_lp", "d_df1", "d_hf", "t60_hf", "f_hf_damp",
    ]},
    # logit (0,1)
    **{n: "logit" for n in ["g_shelf", "c_df"]},
    # plain log for nonneg gains that can exceed 1
    **{n: "log" for n in ["w_loop2", "w_loop3", "g_hf"]},
}

def fwd(name, v):
    t = TRANSFORMS[name]
    if t == "log":
        return math.log(max(v, 1e-9))
    if t == "logit":
        v = min(max(v, 1e-6), 1 - 1e-6)
        return math.log(v / (1 - v))
    return v

def inv(name, u):
    t = TRANSFORMS[name]
    if t == "log":
        return math.exp(u)
    if t == "logit":
        return 1.0 / (1.0 + math.exp(-u))
    return u

STAGES = {
    "A": dict(
        names=["t60", "f_damp", "f_shelf", "g_shelf", "f_hp", "f_lp", "g_hf", "t60_hf"],
        weights=dict(edc=0.5, spec=0.4, dens=0.1, ridge=0.0),
    ),
    "B": dict(
        names=["t_dc", "f_peak", "d_loop1", "d_loop2", "d_loop3", "w_loop2", "w_loop3", "c_df", "d_df1"],
        weights=dict(edc=0.2, spec=0.0, dens=0.3, ridge=0.5),
    ),
    "C": dict(
        names=["t60", "f_damp", "f_shelf", "g_shelf", "f_lp", "t_dc",
               "d_loop1", "w_loop2", "w_loop3", "g_hf", "c_df", "f_hf_damp"],
        weights=dict(edc=0.3, spec=0.25, dens=0.2, ridge=0.25),
    ),
}

# ── IR loading / rendering ───────────────────────────────────────────────────

def load_ref(path=None):
    import wave
    path = path or REF_WAV
    w = wave.open(path)
    assert w.getframerate() == SR and w.getnchannels() == 1 and w.getsampwidth() == 2, (
        f"{path}: need 16-bit mono {SR} Hz (run scripts/prepare_spring_refs.py)"
    )
    x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768.0
    return x

def render(params, seconds=SECONDS):
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(params, f)
        path = f.name
    try:
        out = subprocess.run(
            [BIN, "--params", path, "--sr", str(SR), "--seconds", str(seconds)],
            capture_output=True, check=True,
        ).stdout
    finally:
        os.unlink(path)
    return np.frombuffer(out, dtype="<f4").astype(np.float64)

def normalize(x, sr=SR):
    e = np.sqrt(np.sum(x[: int(4 * sr)] ** 2))
    return x / max(e, 1e-12)

# ── Analyses ─────────────────────────────────────────────────────────────────

def edc_db(x):
    e = np.cumsum((x ** 2)[::-1])[::-1]
    return 10 * np.log10(e / e[0] + 1e-15)

def edc_error(ref, cand, floor_trunc_s):
    er, ec = edc_db(ref), edc_db(cand)
    hi = min(4.0, floor_trunc_s, (min(len(er), len(ec)) - 1) / SR)
    tgrid = np.geomspace(0.010, hi, 80)
    idx = (tgrid * SR).astype(int)
    return float(np.sqrt(np.mean((er[idx] - ec[idx]) ** 2)))

def smooth_spectrum(x, n_sec=1.5):
    seg = x[: int(n_sec * SR)]
    mag = np.abs(np.fft.rfft(seg))
    f = np.fft.rfftfreq(len(seg), 1 / SR)
    # 1/6 octave smoothing on a log-f grid
    fg = np.geomspace(40, 16000, 240)
    out = np.empty_like(fg)
    for i, fc in enumerate(fg):
        lo, hi = fc * 2 ** (-1 / 12), fc * 2 ** (1 / 12)
        m = (f >= lo) & (f < hi)
        out[i] = np.sqrt(np.mean(mag[m] ** 2)) if m.any() else 1e-12
    return fg, 20 * np.log10(out + 1e-12)

def spectrum_error(ref, cand):
    fg, sr_db = smooth_spectrum(ref)
    _, sc_db = smooth_spectrum(cand)
    # match overall level: remove mean difference in the core band
    core = (fg >= 100) & (fg <= 4000)
    off = np.mean(sr_db[core] - sc_db[core])
    d = sr_db - sc_db - off
    band = (fg >= 80) & (fg <= 10000)
    w = np.where(core, 2.0, 1.0)[band]
    return float(np.sqrt(np.average(d[band] ** 2, weights=w)))

def echo_density(x, win_s=0.025, hop_s=0.005, until_s=1.2):
    win = int(win_s * SR)
    hop = int(hop_s * SR)
    n = min(int(until_s * SR), len(x))
    norm = erfc(1 / np.sqrt(2))
    t, d = [], []
    for start in range(0, n - win, hop):
        seg = x[start: start + win]
        s = np.std(seg)
        frac = np.mean(np.abs(seg) > s) if s > 0 else 0.0
        t.append((start + win / 2) / SR)
        d.append(frac / norm)
    return np.array(t), np.array(d)

def density_error(ref, cand):
    t, dr = echo_density(ref)
    tc, dc = echo_density(cand)
    n = min(len(dr), len(dc))
    m = t[:n] <= 1.0
    return float(np.sqrt(np.mean((dr[:n][m] - dc[:n][m]) ** 2)))

def ridge(x, until_s=0.300, nperseg=256):
    seg = x[: int(until_s * SR)]
    f, t, z = stft(seg, SR, nperseg=nperseg, noverlap=nperseg * 3 // 4)
    p = np.abs(z) ** 2
    band = (f >= 100) & (f <= 5000)
    f = f[band]
    p = p[band]
    cent = (p @ t) / np.maximum(p.sum(axis=1), 1e-18)
    return f, cent  # seconds vs Hz

def ridge_error(ref, cand):
    fr, cr = ridge(ref)
    _, cc = ridge(cand)
    return float(np.sqrt(np.mean(((cr - cc) * 1000) ** 2)))  # ms RMS

def noise_floor_trunc(ref):
    """Time where the EDC comes within 8 dB of the tail floor."""
    e = edc_db(ref)
    floor = e[min(int(6.0 * SR), len(e) - int(0.05 * SR))]
    idx = np.argmax(e < floor + 8)
    return idx / SR if idx > 0 else min(4.5, len(ref) / SR * 0.9)

# ── Objective ────────────────────────────────────────────────────────────────

class Objective:
    def __init__(self, ref, weights, base_params, names):
        self.ref = normalize(ref)
        self.weights = weights
        self.base = base_params
        self.names = names
        self.trunc = noise_floor_trunc(ref)
        self.calib = None
        self.evals = 0
        self.best = (np.inf, None)

    def components(self, cand):
        cand = normalize(cand)
        c = {}
        if self.weights.get("edc"):
            c["edc"] = edc_error(self.ref, cand, self.trunc)
        if self.weights.get("spec"):
            c["spec"] = spectrum_error(self.ref, cand)
        if self.weights.get("dens"):
            c["dens"] = density_error(self.ref, cand)
        if self.weights.get("ridge"):
            c["ridge"] = ridge_error(self.ref, cand)
        return c

    def __call__(self, u):
        params = json.loads(json.dumps(self.base))
        for name, ui in zip(self.names, u):
            p_set(params, name, inv(name, ui))
        try:
            cand = render(params)
        except subprocess.CalledProcessError:
            return 1e6
        if not np.all(np.isfinite(cand)):
            return 1e6
        comps = self.components(cand)
        if self.calib is None:
            self.calib = {k: max(v, 1e-9) for k, v in comps.items()}
        total = sum(self.weights[k] * v / self.calib[k] for k, v in comps.items())
        self.evals += 1
        if total < self.best[0]:
            self.best = (total, params)
        if self.evals % 25 == 0:
            pretty = " ".join(f"{k}={v:.3g}" for k, v in comps.items())
            print(f"  eval {self.evals}: J={total:.4f}  {pretty}", flush=True)
        return total

# ── Plots ────────────────────────────────────────────────────────────────────

def make_plots(ref, cand, out_dir):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    os.makedirs(out_dir, exist_ok=True)
    ref = normalize(ref)
    cand = normalize(cand)

    # 1. EDC
    fig, ax = plt.subplots(figsize=(9, 5))
    t = np.arange(len(ref)) / SR
    ax.plot(t, edc_db(ref), label="reference")
    tc = np.arange(len(cand)) / SR
    ax.plot(tc, edc_db(cand), label="candidate")
    ax.set(xlabel="time (s)", ylabel="EDC (dB)", ylim=(-80, 2), title="Energy decay curve")
    ax.grid(alpha=0.3); ax.legend()
    fig.savefig(os.path.join(out_dir, "edc.png"), dpi=110); plt.close(fig)

    # 2. Spectrum
    fig, ax = plt.subplots(figsize=(9, 5))
    fg, sdb = smooth_spectrum(ref)
    _, cdb = smooth_spectrum(cand)
    core = (fg >= 100) & (fg <= 4000)
    cdb = cdb + np.mean(sdb[core] - cdb[core])
    ax.semilogx(fg, sdb, label="reference")
    ax.semilogx(fg, cdb, label="candidate")
    ax.set(xlabel="freq (Hz)", ylabel="mag (dB)", title="Magnitude spectrum (1/6 oct smoothed, first 1.5 s)")
    ax.grid(alpha=0.3, which="both"); ax.legend()
    fig.savefig(os.path.join(out_dir, "spectrum.png"), dpi=110); plt.close(fig)

    # 3. Echo density
    fig, ax = plt.subplots(figsize=(9, 5))
    t1, d1 = echo_density(ref)
    t2, d2 = echo_density(cand)
    ax.plot(t1, d1, label="reference")
    ax.plot(t2, d2, label="candidate")
    ax.set(xlabel="time (s)", ylabel="normalized echo density", title="Abel–Huang echo density")
    ax.grid(alpha=0.3); ax.legend()
    fig.savefig(os.path.join(out_dir, "echo_density.png"), dpi=110); plt.close(fig)

    # 4. Spectrogram + dispersion ridge
    fig, axes = plt.subplots(1, 2, figsize=(13, 5), sharey=True)
    for ax, x, name in [(axes[0], ref, "reference"), (axes[1], cand, "candidate")]:
        seg = x[: int(0.300 * SR)]
        f, tt, z = stft(seg, SR, nperseg=256, noverlap=192)
        ax.pcolormesh(tt * 1000, f, 20 * np.log10(np.abs(z) + 1e-9),
                      vmin=-110, vmax=-30, cmap="magma", shading="auto")
        fr, cr = ridge(x)
        ax.plot(cr * 1000, fr, "c-", lw=1.5, label="energy centroid")
        ax.set(title=f"{name}", xlabel="time (ms)", ylim=(0, 6000))
        ax.legend(loc="upper right")
    axes[0].set(ylabel="freq (Hz)")
    fig.suptitle("Dispersion trajectory (first 300 ms)")
    fig.savefig(os.path.join(out_dir, "spectrogram_ridge.png"), dpi=110); plt.close(fig)

# ── Commands ─────────────────────────────────────────────────────────────────

def cmd_analyze(args):
    ref = load_ref(args.ref)
    refn = normalize(ref)
    print(f"len {len(ref)/SR:.2f}s  peak@{np.abs(ref).argmax()/SR*1000:.1f}ms")
    print(f"noise-floor truncation at {noise_floor_trunc(ref):.2f}s")
    fr, cr = ridge(refn)
    for f0 in (150, 300, 600, 1200, 2400, 4800):
        i = np.argmin(np.abs(fr - f0))
        print(f"  ridge centroid @ {fr[i]:5.0f} Hz: {cr[i]*1000:6.1f} ms")
    t, d = echo_density(refn)
    for ts in (0.1, 0.3, 0.6, 1.0):
        i = np.argmin(np.abs(t - ts))
        print(f"  echo density @ {t[i]:.2f}s: {d[i]:.2f}")
    e = edc_db(refn)
    for db in (-10, -20, -30, -40):
        print(f"  EDC {db} dB @ {np.argmax(e < db)/SR:.2f}s")

def load_params(path):
    if path and os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return json.loads(json.dumps(DEFAULT_PARAMS))

def cmd_plots(args):
    ref = load_ref(args.ref)
    params = load_params(args.params)
    cand = render(params)
    out = os.path.join(OUT_DIR, args.tag)
    make_plots(ref, cand, out)
    obj = Objective(ref, dict(edc=1, spec=1, dens=1, ridge=1), params, [])
    comps = obj.components(cand)
    print(json.dumps(comps, indent=2))
    print(f"plots -> {out}")

def cmd_optimize(args):
    ref = load_ref(args.ref)
    base = load_params(args.params)
    stage = STAGES[args.stage]
    names = stage["names"]
    obj = Objective(ref, stage["weights"], base, names)
    u0 = np.array([fwd(n, p_get(base, n)) for n in names])

    starts = [u0]
    for s in range(args.multistart):
        rng = np.random.default_rng(1234 + s)
        starts.append(u0 + rng.normal(0, 0.15, size=u0.shape))

    best = (np.inf, None)
    for si, start in enumerate(starts):
        print(f"— start {si} —", flush=True)
        res = minimize(obj, start, method="Nelder-Mead",
                       options=dict(adaptive=True, maxiter=args.maxiter,
                                    xatol=1e-3, fatol=1e-4))
        print(f"  start {si}: J={res.fun:.4f} after {res.nfev} evals", flush=True)
        if res.fun < best[0]:
            best = (res.fun, res.x)

    tag = (
        os.path.splitext(os.path.basename(args.ref))[0] + "_" if args.ref else ""
    )
    params = json.loads(json.dumps(base))
    for name, ui in zip(names, best[1]):
        p_set(params, name, inv(name, ui))
    out_json = args.out or os.path.join(OUT_DIR, f"best_params_{tag}{args.stage}.json")
    os.makedirs(os.path.dirname(out_json), exist_ok=True)
    with open(out_json, "w") as f:
        json.dump(params, f, indent=2)
    print(f"best J={best[0]:.4f} -> {out_json}")

    cand = render(params)
    out = os.path.join(OUT_DIR, f"{tag}stage_{args.stage}")
    make_plots(ref, cand, out)
    full = Objective(ref, dict(edc=1, spec=1, dens=1, ridge=1), params, [])
    print(json.dumps(full.components(cand), indent=2))
    print(f"plots -> {out}")

def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("analyze")
    p.add_argument("--ref")
    p = sub.add_parser("plots")
    p.add_argument("--ref")
    p.add_argument("--params")
    p.add_argument("--tag", default="manual")
    p = sub.add_parser("optimize")
    p.add_argument("--stage", choices=list(STAGES), required=True)
    p.add_argument("--ref")
    p.add_argument("--params")
    p.add_argument("--out")
    p.add_argument("--maxiter", type=int, default=400)
    p.add_argument("--multistart", type=int, default=0)
    args = ap.parse_args()
    {"analyze": cmd_analyze, "plots": cmd_plots, "optimize": cmd_optimize}[args.cmd](args)

if __name__ == "__main__":
    main()
