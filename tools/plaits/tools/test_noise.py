import sys, os, json; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plh, stats_common as S, lcg as L
from exact_window import exact_report
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
N = 240000  # 2 draws per sample, 2.5 s
lcg = os.path.join(plh.WORK, "tests", "data_lcg", "noise_lcg.json")
if not os.path.exists(lcg) or json.load(open(lcg))["shape"][0] != N:
    os.makedirs(os.path.dirname(lcg), exist_ok=True)
    json.dump({"shape": [N, 1], "data": [float(x) for x in L.draws(N)]}, open(lcg, "w"), separators=(",", ":"))
# Exact check: feed the reference LCG's draws (noise 0 takes draws 24b..24b+11
# of block b, noise 1 the next 12) into the engine core.
ex = plh.EngineTest("noise_exact", ["engines/noise.lisp"],
    "(noise-core bstart bphase rise patched note timbre morph harmonics "
    "(peek noise_lcg (+ (* 24 (floor (/ n_now 12))) bphase) 0) "
    "(peek noise_lcg (+ (* 24 (floor (/ n_now 12))) bphase 12) 0))",
    data_files=[lcg],
    extra_defs=f'(def noise_lcg (tensor @shape [{N} 1] @file "noise_lcg.json"))\n')
print("-- exact (reference LCG draws injected), 2.4 s; plh.compare then lag-0 windows")
cases = [("h0.5 t0.5 m0.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
         ("h0 t0 m0", dict(harmonics=0, timbre=0, morph=0)),
         ("h1 t1 m1", dict(harmonics=1, timbre=1, morph=1)),
         ("h0.2 t0.8 m0.3", dict(harmonics=0.2, timbre=0.8, morph=0.3)),
         ("h0.8 t0.3 m0.9", dict(harmonics=0.8, timbre=0.3, morph=0.9)),
         ("h0.6 t0.95 m0", dict(harmonics=0.6, timbre=0.95, morph=0)),
         ("sweep", dict(note=24, note_end=96, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1)),
         ("gate retrig (sync)", dict(harmonics=0.3, timbre=0.3, morph=0.6, gate=0.5, retrig=0.25)),
         ("unpatched", dict(harmonics=0.3, timbre=0.3, morph=0.6, trig=0))]
for lab, kw in cases:
    kw = dict(dict(engine=17, note=48, seconds=2.4), **kw)
    ex.compare(lab, **kw)
    exact_report(ex, lab, **kw)
print("-- statistics with the instrument's own noise (ref/port)")
t = plh.EngineTest("noise", ["engines/noise.lisp"],
    "(plaits-noise bstart bphase rise high patched note timbre morph harmonics accent)")
for lab, kw in cases:
    S.stats(t, lab, **dict(dict(engine=17, note=48, seconds=4), **kw))
t.cpu()
