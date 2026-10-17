"""Statistics comparison for noise-driven Plaits engines."""
import numpy as np

def centroid(x, sr=48000):
    X = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    f = np.fft.rfftfreq(len(x), 1 / sr)
    return float(np.sum(f * X) / max(np.sum(X), 1e-12))

def band_profile(x, sr=48000, nb=8):
    """Fraction of energy in octave bands from 94 Hz up (8 bands)."""
    X = np.abs(np.fft.rfft(x)) ** 2
    f = np.fft.rfftfreq(len(x), 1 / sr)
    edges = [0] + [93.75 * 2 ** k for k in range(nb)] + [sr / 2]
    e = np.array([X[(f >= a) & (f < b)].sum() for a, b in zip(edges[:-1], edges[1:])])
    return e / max(e.sum(), 1e-20)

def env(x, win=4800):
    n = len(x) // win
    return np.sqrt(np.mean(x[:n * win].reshape(n, win) ** 2, axis=1))

def stats(t, label, **kw):
    kw.setdefault("seconds", 2.0)
    r = t.ref(**kw); p = t.port(**kw)
    n = min(len(r), len(p)); r, p = r[:n], p[:n]
    out = []
    for ch, nm in ((0, "out"), (1, "aux")):
        rr, pr = r[:, ch], p[:, ch]
        rms_r, rms_p = np.sqrt(np.mean(rr ** 2)), np.sqrt(np.mean(pr ** 2))
        c_r, c_p = centroid(rr), centroid(pr)
        er, ep = env(rr), env(pr)
        envd = float(np.sqrt(np.mean((er - ep) ** 2)) / max(np.sqrt(np.mean(er ** 2)), 1e-9))
        br, bp = band_profile(rr), band_profile(pr)
        bandd = float(np.abs(br - bp).sum() / 2)
        out.append(f"{nm}: rms {rms_r:.4f}/{rms_p:.4f} cent {c_r:6.0f}/{c_p:6.0f}Hz env {envd:.3f} band {bandd:.3f}")
    print(f"{label:24s} " + "  ".join(out), flush=True)
    return r, p
