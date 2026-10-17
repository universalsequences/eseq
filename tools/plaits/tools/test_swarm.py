import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh, stats_common as S
t = plh.EngineTest("swarm", ["engines/swarm.lisp"],
    "(plaits-swarm bstart bphase rise high patched note timbre morph harmonics accent)")
print("-- exact (spread 0, all grains below size 1: no random dependency)")
for tb, m in [(0.5, 1.0), (0.0, 0.8), (1.0, 0.75)]:
    t.compare(f"h0 t{tb} m{m}", engine=16, note=48, harmonics=0, timbre=tb, morph=m)
t.compare("h0 sweep note", engine=16, note=24, note_end=100, harmonics=0, timbre=0.5, morph=0.9, seconds=3)
t.compare("h0 m1 gate retrig", engine=16, note=48, harmonics=0, morph=1, gate=0.5, retrig=0.25)
print("-- statistics (ref/port)")
for h, tb, m in [(0.5, 0.5, 0.5), (1.0, 1.0, 1.0), (0.3, 0.2, 0.0), (0.8, 0.7, 0.3), (0.0, 0.5, 0.2), (1.0, 0.0, 0.0)]:
    S.stats(t, f"h{h} t{tb} m{m}", engine=16, note=48, harmonics=h, timbre=tb, morph=m, seconds=4)
S.stats(t, "sweep", engine=16, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=1, morph_end=0, seconds=4)
S.stats(t, "burst gate retrig", engine=16, note=48, harmonics=0.6, timbre=0.4, morph=0.3, gate=0.5, retrig=0.25, seconds=4)
S.stats(t, "unpatched", engine=16, note=48, harmonics=0.6, timbre=0.4, morph=0.3, trig=0, seconds=4)
t.cpu()
