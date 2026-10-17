import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
from drumstats import stats
t = plh.EngineTest("sd", ["engines/sd.lisp"],
    "(plaits-sd bstart bphase rise high patched note timbre morph harmonics accent)")
E = dict(engine=22)
cases = [
    ("h0 t0 m0 (no noise)", dict(harmonics=0, timbre=0, morph=0)),
    ("h0 t.5 m.5 (no noise)", dict(harmonics=0, timbre=0.5, morph=0.5)),
    ("h0 t1 m1 n36 (no noise)", dict(harmonics=0, timbre=1, morph=1, note=36)),
    ("h0 t.8 m.3 n60 (no noise)", dict(harmonics=0, timbre=0.8, morph=0.3, note=60)),
    ("h.5 t.5 m.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
    ("h1 t.2 m.8", dict(harmonics=1, timbre=0.2, morph=0.8)),
]
for lab, kw in cases:
    r, p, _ = t.compare(lab, gate=0.1, retrig=0.25, **E, **kw)
    if kw["harmonics"] > 0: stats(lab, r, p)
r, p, _ = t.compare("sweep retrig", note=30, note_end=70, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1, gate=0.05, retrig=0.2, seconds=3, **E)
stats("sweep", r, p, n=10)
r, p, _ = t.compare("gate .5 single h.3", gate=0.5, harmonics=0.3, timbre=0.6, morph=0.6, **E)
stats("", r, p)
for lab, kw in [("unpatched h0 t.5 m.5", dict(harmonics=0, timbre=0.5, morph=0.5)),
                ("unpatched h.6 t.9 m1", dict(harmonics=0.6, timbre=0.9, morph=1.0))]:
    r, p, _ = t.compare(lab, trig=0, **E, **kw)
    stats(lab, r, p)
if "cpu" in sys.argv: t.cpu(gate=0.1, retrig=0.25)
