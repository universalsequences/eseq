import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import numpy as np
import plh
t = plh.EngineTest("bd", ["engines/bd.lisp"],
    "(plaits-bd bstart bphase rise high patched note timbre morph harmonics accent)")
def stats(label, r, p):
    # envelope / spectral statistics for the noisy AUX channel
    def cen(x):
        X = np.abs(np.fft.rfft(x * np.hanning(len(x))))
        f = np.fft.rfftfreq(len(x), 1 / 48000)
        return float((X * f).sum() / max(X.sum(), 1e-12))
    seg = 4800
    er = [float(np.sqrt(np.mean(r[i:i+seg, 1]**2))) for i in range(0, len(r) - seg + 1, seg)]
    ep = [float(np.sqrt(np.mean(p[i:i+seg, 1]**2))) for i in range(0, len(p) - seg + 1, seg)]
    print(f"   aux stats {label}: centroid ref {cen(r[:,1]):.0f} port {cen(p[:,1]):.0f} Hz; "
          f"env100ms ref {' '.join(f'{v:.3f}' for v in er[:6])} | port {' '.join(f'{v:.3f}' for v in ep[:6])}")
cases = [
    ("h0 t0 m0", dict(harmonics=0, timbre=0, morph=0)),
    ("h.5 t.5 m.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
    ("h1 t1 m1", dict(harmonics=1, timbre=1, morph=1)),
    ("h.2 t.8 m.3 n36", dict(harmonics=0.2, timbre=0.8, morph=0.3, note=36)),
    ("h.8 t.3 m.8 n60", dict(harmonics=0.8, timbre=0.3, morph=0.8, note=60)),
]
for lab, kw in cases:
    r, p, _ = t.compare(lab, engine=21, gate=0.1, retrig=0.25, **kw)
    stats(lab, r, p)
r, p, _ = t.compare("sweep retrig", engine=21, note=30, note_end=70, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1, gate=0.05, retrig=0.2, seconds=3)
stats("sweep", r, p)
for lab, kw in [("unpatched h.3 m.5", dict(harmonics=0.3, timbre=0.5, morph=0.5)),
                ("unpatched h.9 m1 t.2", dict(harmonics=0.9, timbre=0.2, morph=1.0))]:
    r, p, _ = t.compare(lab, engine=21, trig=0, **kw)
    stats(lab, r, p)
if "cpu" in sys.argv: t.cpu(gate=0.5, retrig=0.25)
