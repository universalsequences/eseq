#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2", "scipy>=1.14", "matplotlib>=3.9"]
# ///
"""Grampian analysis: never average references or fit to peak-trimmed amp clicks.

Run with uv run scripts/grampian_tune.py. `compare` measures the Rust renderer;
`fit` uses the exact linear transfer function for fast offline identification.
The analytical model is independently checked against Rust by `verify`.
Only the primary Grampian is optimized. The related filtered capture is a
separate tonal cross-check, NOT an independent tank or an averaged target.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess

import numpy as np
from scipy.ndimage import gaussian_filter1d
from scipy.optimize import least_squares, minimize
from scipy.signal import find_peaks, stft

ROOT = Path(__file__).resolve().parents[1]
SR = 48000
SECONDS = 5.0
NFFT = 262144
# Matches docs/grampian-spring.md and grampian_audition.py: the documented
# workflow only ever builds the release binary.
BIN = ROOT / "target/release/spring_tune"
BUILD_HINT = "cargo build --release -p sequencer --bin spring_tune"
MANIFEST = ROOT / "content/impulses/spring-references.json"
BANDS = [(250, 500), (500, 1000), (1000, 2000), (2000, 4000), (4000, 6000), (6000, 9000)]
SPECTRAL_BANDS = [(80, 250)] + BANDS + [(9000, 12000), (12000, 16000)]


def decode(path, sr=SR):
    raw = subprocess.check_output(["ffmpeg", "-v", "error", "-i", str(path),
                                   "-f", "f32le", "-ac", "1", "-ar", str(sr), "-"])
    return np.frombuffer(raw, dtype="<f4").astype(float)


def reference(name="grampian-sweep"):
    manifest = json.loads(MANIFEST.read_text())
    entry = next(e for e in manifest["references"] if e["id"] == name)
    path = ROOT / "content/impulses" / entry["path"]
    if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
        raise ValueError(f"reference changed: {path}")
    return decode(path), entry


def edge(hz, highpass, sr, z, q=1/np.sqrt(2)):
    w = 2 * np.pi * min(max(hz, 10), sr * .45) / sr
    alpha = np.sin(w) / (2*q)
    c = np.cos(w)
    b0 = (1 + c if highpass else 1 - c) * .5 / (1 + alpha)
    b1 = (-2 if highpass else 2) * b0
    a1, a2 = -2*c/(1+alpha), (1-alpha)/(1+alpha)
    return (b0 + b1*z + b0*z*z) / (1 + a1*z + a2*z*z)


def analytical(params, sr=SR, nfft=NFFT, tension=.5):
    """Exact stationary transfer of grampian.rs, including fractional reads.

    This is an analysis acceleration, not an alternative production renderer.
    No circular-tail correction: verify() bounds the residual wrap explicitly.
    """
    z = np.exp(-2j * np.pi * np.fft.rfftfreq(nfft))
    total = np.zeros_like(z)
    scale = 2 ** (2 * (tension - .5))
    for p in params["paths"]:
        ap = np.ones_like(z)
        ap_dc = 0.0
        for d in p["dispersion"]:
            r = np.exp(-np.pi * d["bandwidth_hz"] * scale / sr)
            hz = min(d["pole_hz"] * scale, sr * .40)
            a1, a2 = -2*r*np.cos(2*np.pi*hz/sr), r*r
            ap *= ((a2 + a1*z + z*z) / (1 + a1*z + a2*z*z)) ** d["sections"]
            ap_dc += d["sections"] * 2*(1-a2)/(1+a1+a2)
        forward = np.clip(p["forward_s"] / scale * sr, 2, 65533)
        back = np.clip(p["return_s"] / scale * sr, 2, 65533)
        def delay(samples):
            whole = int(samples)
            u = samples - whole
            weights = [-u*(1-u)*(2-u)/6, (1+u)*(1-u)*(2-u)/2,
                       (1+u)*u*(2-u)/2, -(1+u)*u*(1-u)/6]
            return z**(whole-1) * sum(c*z**i for i, c in enumerate(weights))
        scatter_delay = np.clip(p["scatter_s"] / scale * sr, 2, 65533) if p["scatter_s"] else 0.0
        scatter = (delay(scatter_delay)-p["scatter"])/(1-p["scatter"]*delay(scatter_delay)) if scatter_delay else 1.0
        scatter_dc = scatter_delay*(1+p["scatter"])/(1-p["scatter"])
        roundtrip = (forward + back + scatter_dc + 1 + 2*ap_dc) / sr
        fb = min(10 ** (-3*roundtrip/(p["t60_s"]*scale**(-.4))), .9995)
        damp = 1 - np.exp(-2*np.pi*min(p["damping_hz"]*scale, sr*.45)/sr)
        damping = damp / (1 - (1-damp)*z)
        shelf = 1 - np.exp(-2*np.pi*p["shelf_hz"]*scale/sr)
        damping *= p["shelf_gain"] + (1-p["shelf_gain"])*shelf/(1-(1-shelf)*z)
        feed = edge(p["highpass_hz"]*scale, True, sr, z, p["highpass_q"])
        for q in (.5097956, .6013449, .8999762, 2.5629154):
            feed *= edge(p["lowpass_hz"]*scale, False, sr, z, q)
        first = ap * delay(forward)
        total += p["gain"] * feed * first / (1 - fb*first*ap*delay(back)*scatter*z*damping)
    return np.fft.irfft(total, nfft)


def rust_render(voice, params=None, sr=SR, seconds=SECONDS, tension=.5):
    args = [str(BIN), "--voice", voice, "--sr", str(sr), "--seconds", str(seconds),
            "--amp", "1", "--tension", str(tension)]
    if params:
        args += ["--params", str(params)]
    if not Path(args[0]).is_file():
        raise SystemExit(f"spring_tune binary not found at {args[0]}; build it with "
                         f"`{BUILD_HINT}` or pass --bin")
    proc = subprocess.run(args, capture_output=True, check=False)
    if proc.returncode != 0:
        stderr = proc.stderr.decode(errors="replace").strip()
        raise SystemExit(f"{args[0]} failed (exit {proc.returncode}); a stale binary "
                         f"wants a rebuild with `{BUILD_HINT}`:\n{stderr}")
    return np.frombuffer(proc.stdout, dtype="<f4").astype(float)


def features(x):
    x = np.pad(x[:int(SECONDS*SR)], (0, max(0, int(SECONDS*SR)-len(x))))
    x /= max(np.sqrt(np.sum(x*x)), 1e-15)
    # Full time-frequency surface: actual packets remain visible; a temporal
    # centroid cannot substitute for this. 10.7ms Hann, 1ms hop, first 300ms.
    f, t, z = stft(x[:int(.31*SR)], SR, nperseg=512, noverlap=464)
    band = (f >= 500) & (f <= 8500)
    early = gaussian_filter1d(abs(z[band]), 1, axis=1)
    # Fixed per-frequency reference scaling is applied by Objective, not an
    # independent candidate normalization that could hide missing bands.
    f2, t2, z2 = stft(x, SR, nperseg=2048, noverlap=1536)
    power = abs(z2)**2
    # Include out-of-band energy: otherwise the HF-path optimizer can hide
    # unwanted brightness above the last decay band, or sub-bass below it.
    spectrum = [power[(f2 >= lo) & (f2 < hi)].sum() for lo, hi in SPECTRAL_BANDS]
    edcs = []
    for lo, hi in BANDS:
        e = power[(f2 >= lo) & (f2 < hi)].sum(axis=0)
        # Remove estimated stationary tail noise, never fit its integrated
        # energy as spring sustain. Stop comparisons at reference confidence.
        floor = np.median(e[-max(1, int(.3*SR/512)):])
        tail = np.cumsum(np.maximum(e-floor, 0)[::-1])[::-1]
        edcs.append(10*np.log10(tail / max(tail[0], 1e-30) + 1e-12))
    return early, np.array(spectrum), np.array(edcs), t2


class Objective:
    def __init__(self, ref):
        self.early, self.spectrum, self.edcs, self.times = features(ref.copy())
        self.scale = np.maximum(self.early.max(axis=1, keepdims=True), self.early.max()*.02)
        self.mask = (self.edcs > -30) & (self.times[None, :] < 2.0) & (self.times[None, :] > .04)

    def components(self, x):
        early, spectrum, edcs, _ = features(x.copy())
        # Compressed magnitudes preserve packet location without sample-phase
        # matching. Zero/absent packets are penalized, not skipped.
        packets = np.mean((np.sqrt(early/self.scale) - np.sqrt(self.early/self.scale))**2)
        spec = np.sqrt(np.mean((10*np.log10((spectrum+1e-20)/(self.spectrum+1e-20)))**2))
        decay = np.sqrt(np.mean((edcs[self.mask] - self.edcs[self.mask])**2))
        return dict(packets=float(packets), spectrum_db=float(spec), decay_db=float(decay))

    def loss(self, x):
        c = self.components(x)
        return c["packets"]/.02 + (c["spectrum_db"]/4)**2 + (c["decay_db"]/4)**2


def plot(ref, candidates, out):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    out.mkdir(parents=True, exist_ok=True)
    items = [("reference", ref)] + list(candidates.items())
    fig, axes = plt.subplots(len(items), 1, figsize=(14, 3.5*len(items)), sharex=True, sharey=True)
    for ax, (name, x) in zip(np.atleast_1d(axes), items):
        f, t, z = stft(x[:int(.31*SR)], SR, nperseg=512, noverlap=464)
        db = 20*np.log10(abs(z)/max(abs(z).max(), 1e-20)+1e-9)
        ax.pcolormesh(t*1000, f, db, vmin=-50, vmax=0, cmap="magma", shading="auto")
        ax.set(title=name, ylim=(0, 9000), xlim=(0, 300), ylabel="Hz")
    axes[-1].set_xlabel("ms from capture start (no peak alignment)")
    fig.tight_layout(); fig.savefig(out/"packets.png"); plt.close(fig)
    fig, axes = plt.subplots(2, 3, figsize=(14, 7))
    for name, x in items:
        _, _, curves, t = features(x.copy())
        for ax, band, curve in zip(axes.flat, BANDS, curves):
            ax.plot(t, curve, label=name)
            ax.set(title=f"{band[0]}–{band[1]} Hz", xlim=(0, 2), ylim=(-45, 0))
    axes[0, 0].legend(); fig.tight_layout(); fig.savefig(out/"band-decay.png"); plt.close(fig)


def identify(args):
    """Track the first TWO increasing-frequency packets, not a centroid.

    Local peak prominence and monotone continuation keep the earlier HF
    precursor out of the torsional tracks. This is a curated Grampian
    identifier, not a claim to identify arbitrary reverb recordings.
    """
    x, entry = reference(args.reference)
    frequencies = np.array([1000, 1500, 2000, 2500, 3000, 3500, 4000, 4500,
                            5000, 5250, 5500, 5750, 6000])
    tracks = [[], []]
    for hz in frequencies:
        window = 512 if hz < 5000 else 1024
        f, t, z = stft(x[:int(.15*SR)], SR, nperseg=window, noverlap=window-12)
        e = (abs(z[(f >= hz-100) & (f <= hz+100)])**2).sum(axis=0)
        peaks, _ = find_peaks(e, prominence=e.max()*.1, distance=int(.005*SR/12))
        previous = .010 if not tracks[0] else tracks[0][-1]/1000-.001
        first = next((i for i in peaks if t[i] >= previous), None)
        previous_second = .015 if not tracks[1] else tracks[1][-1]/1000-.001
        second = next((i for i in peaks if first is not None and
                       t[i] >= max(t[first]+.005, previous_second)), None)
        if first is None or second is None:
            raise ValueError(f"cannot continue both chirps at {hz} Hz; inspect the spectrogram")
        tracks[0].append(float(t[first]*1000)); tracks[1].append(float(t[second]*1000))
    fits = []
    for target in tracks:
        def delays(v):
            result = np.full(len(frequencies), v[-1])
            w = 2*np.pi*frequencies/SR
            for hz, bw in zip(v[:3], v[3:6]):
                r = np.exp(-np.pi*bw/SR); theta = 2*np.pi*hz/SR
                result += 40000/SR * ((1-r*r)/(1+r*r-2*r*np.cos(w-theta)) +
                                      (1-r*r)/(1+r*r-2*r*np.cos(w+theta)))
            return result
        fit = least_squares(lambda v: delays(v)-target,
            [4500, 5600, 6100, 2800, 1300, 600, target[0]-3],
            bounds=([2000, 4000, 6000, 100, 100, 100, 0],
                    [7000, 8000, 9000, 8000, 8000, 8000, 30]), max_nfev=5000)
        fits.append(dict(forward_s=float(fit.x[-1]/1000),
            dispersion=[dict(sections=40, pole_hz=float(hz), bandwidth_hz=float(bw))
                        for hz, bw in zip(fit.x[:3], fit.x[3:6])],
            rms_ms=float(np.sqrt(np.mean(fit.fun**2)))))
    args.out.mkdir(parents=True, exist_ok=True)
    result = dict(reference=entry, frequencies_hz=frequencies.tolist(), tracks_ms=tracks, fits=fits)
    (args.out/"identified.json").write_text(json.dumps(result, indent=2)+"\n")
    print(json.dumps(result, indent=2))


def verify(args):
    params = json.loads(args.params.read_text())
    results = []
    for sr in (44100, 48000, 96000):
        rust = rust_render("grampian", args.params, sr, 3)
        model = analytical(params, sr, nfft=1 << int(np.ceil(np.log2(sr*12)) ))[:len(rust)]
        error = np.linalg.norm(model-rust)/np.linalg.norm(rust)
        results.append(dict(sr=sr, relative_l2=float(error)))
        if error > .002:
            raise AssertionError(results)
    print(json.dumps(results, indent=2))


def ablate(args):
    ref, entry = reference()
    params = json.loads(args.params.read_text())
    objective = Objective(ref)
    variants = {"full": params}
    for name in ("no-scattering", "no-precursor", "no-dispersion"):
        p = copy.deepcopy(params)
        if name == "no-scattering":
            for path in p["paths"]: path["scatter_s"] = 0.0
        elif name == "no-precursor": p["paths"][2]["gain"] = 0.0
        else:
            for path in p["paths"]:
                for d in path["dispersion"]: d["sections"] = 0
        variants[name] = p
    renders = {name: analytical(p) for name, p in variants.items()}
    result = {name: objective.components(x) for name, x in renders.items()}
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out/"ablations.json").write_text(json.dumps(dict(reference=entry, metrics=result), indent=2)+"\n")
    plot(ref, renders, args.out)
    print(json.dumps(result, indent=2))


def compare(args):
    ref, entry = reference(args.reference)
    candidates = {"v1": rust_render("king-tubby-v1"), "grampian": rust_render("grampian", args.params)}
    objective = Objective(ref)
    result = {name: objective.components(x) for name, x in candidates.items()}
    plot(ref, candidates, args.out)
    (args.out/"metrics.json").write_text(json.dumps(dict(reference=entry, metrics=result), indent=2)+"\n")
    print(json.dumps(result, indent=2))


def fit(args):
    ref, entry = reference()
    base = json.loads(args.params.read_text())
    if args.geometry:
        geometry = json.loads(args.geometry.read_text())
        if geometry["reference"]["sha256"] != entry["sha256"]:
            raise ValueError("geometry was identified from a different reference")
        for p, fit in zip(base["paths"][:2], geometry["fits"]):
            p["dispersion"] = fit["dispersion"]
            p["forward_s"] = fit["forward_s"]
    # Energy normalization cannot identify a common output gain. Fix path 0
    # to unity and optimize only relative pickup gains.
    gain = base["paths"][0]["gain"]
    for p in base["paths"]: p["gain"] /= gain
    objective = Objective(ref)
    # Keep first-arrival dispersion fixed in the voicing stage. Recurrence
    # delays may move only 2 ms around the measured geometry.
    names = ["return_s", "t60_s", "damping_hz", "highpass_hz", "lowpass_hz", "gain"]
    bounds = {"return_s": (.005, .10), "t60_s": (.1, 10), "damping_hz": (500, 18000),
              "highpass_hz": (100, 10000), "lowpass_hz": (1500, 18000), "gain": (.01, 2.5),
              "scatter": (.05, .7), "shelf_hz": (100, 3000), "shelf_gain": (.3, 1), "highpass_q": (.5, 8)}
    keys = [(i, key) for i in range(3) for key in names if (i, key) != (0, "gain")] + [
        (i, key) for i in range(2) for key in ("scatter", "shelf_hz", "shelf_gain")] + [(2, "highpass_q")]
    if args.precursor_only: keys = [(i, key) for i, key in keys if i == 2]
    x0 = [np.log(base["paths"][i][key]) for i, key in keys]
    return_bounds = [(.0085, .0125), (.008, .012), (.008, .012)]
    box = [(np.log(return_bounds[i][0]), np.log(return_bounds[i][1]))
           if key == "return_s" else (np.log(bounds[key][0]), np.log(bounds[key][1]))
           for i, key in keys]
    best = [float("inf"), None]
    count = 0
    def loss(v):
        nonlocal count
        p = copy.deepcopy(base)
        for (i, key), value in zip(keys, v): p["paths"][i][key] = float(np.exp(value))
        if any(q["highpass_hz"] >= q["lowpass_hz"] for q in p["paths"]): return 1e6
        value = objective.loss(analytical(p))
        count += 1
        if value < best[0]: best[:] = [value, p]
        if count % 25 == 0: print(f"eval {count}: {value:.5f} best {best[0]:.5f}", flush=True)
        return value
    minimize(loss, x0, method="Nelder-Mead", bounds=box,
             options=dict(maxiter=args.iterations, adaptive=True, xatol=.001, fatol=.0001))
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out/"params.json").write_text(json.dumps(best[1], indent=2)+"\n")
    report = dict(reference=entry, seed_sha256=hashlib.sha256(args.params.read_bytes()).hexdigest(),
                  iterations=args.iterations, evaluations=count, loss=best[0],
                  components=objective.components(analytical(best[1])),
                  geometry=str(args.geometry) if args.geometry else "seed geometry")
    (args.out/"fit-report.json").write_text(json.dumps(report, indent=2)+"\n")
    print("best", best[0])
    plot(ref, {"fit": analytical(best[1])}, args.out)


def main():
    global BIN
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bin", type=Path, default=BIN)
    sub = ap.add_subparsers(dest="command", required=True)
    for command in ("identify", "verify", "compare", "fit", "ablate"):
        p = sub.add_parser(command)
        if command != "identify": p.add_argument("--params", type=Path, required=True)
        p.add_argument("--out", type=Path, default=ROOT/"tuning_out/grampian-v2")
        if command in ("identify", "compare"): p.add_argument("--reference", default="grampian-sweep")
        if command == "fit":
            p.add_argument("--iterations", type=int, default=400)
            p.add_argument("--geometry", type=Path)
            p.add_argument("--precursor-only", action="store_true")
    args = ap.parse_args(); BIN = args.bin
    globals()[args.command](args)


if __name__ == "__main__":
    main()
