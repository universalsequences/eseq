import sys, os, json; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plh, stats_common as S, lcg as L
from exact_window import exact_report
from gen_particle_exact import gen
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
N = 340000  # ~80 draws per block -> ~1.05 s
lcg = os.path.join(plh.WORK, "tests", "data_lcg", "particle_lcg.json")
if not os.path.exists(lcg) or json.load(open(lcg))["shape"][0] != N:
    os.makedirs(os.path.dirname(lcg), exist_ok=True)
    json.dump({"shape": [N, 1], "data": [float(x) for x in L.draws(N)]}, open(lcg, "w"), separators=(",", ":"))
defs, call = gen()
ex = plh.EngineTest("particle_exact", ["engines/particle.lisp"],
    "(tuple (particle-post pt_plp pt_dif) pt_aux)", data_files=[lcg],
    extra_defs=f'(def particle_lcg (tensor @shape [{N} 1] @file "particle_lcg.json"))\n' + defs
               + f"(def (pt_plp pt_aux pt_dif) {call})\n")
cases = [("h0.5 t0.5 m0.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
         ("h0 t0 m0", dict(harmonics=0, timbre=0, morph=0)),
         ("h1 t1 m1", dict(harmonics=1, timbre=1, morph=1)),
         ("h0.3 t0.8 m0.2 (diff)", dict(harmonics=0.3, timbre=0.8, morph=0.2)),
         ("h0.8 t0.6 m0.8", dict(harmonics=0.8, timbre=0.6, morph=0.8)),
         ("h0.5 t1 m0 (full diff)", dict(harmonics=0.5, timbre=1, morph=0)),
         ("sweep", dict(note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1)),
         ("gate retrig (sync)", dict(harmonics=0.4, timbre=0.3, morph=0.3, gate=0.05, retrig=0.25)),
         ("unpatched", dict(harmonics=0.4, timbre=0.6, morph=0.3, trig=0))]
print("-- exact (reference LCG draws injected in C++ consumption order), 1.0 s")
for lab, kw in cases:
    kw = dict(dict(engine=18, note=48, seconds=1.0), **kw)
    ex.compare(lab, **kw)
    exact_report(ex, lab, **kw)
print("-- statistics with the instrument's own noise (ref/port), 4 s")
t = plh.EngineTest("particle", ["engines/particle.lisp"],
    "(plaits-particle bstart bphase rise high patched note timbre morph harmonics accent 1)")
for lab, kw in cases:
    S.stats(t, lab, **dict(dict(engine=18, note=48, seconds=4), **kw))
t.cpu()
inactive = plh.EngineTest("particle_inactive", ["engines/particle.lisp"],
    "(plaits-particle bstart bphase rise high patched note timbre morph harmonics accent 0)")
print("active=0:", end=" "); inactive.cpu()
