import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("terrain", ["engines/terrain.lisp"],
    "(plaits-terrain bstart bphase rise high patched note timbre morph harmonics accent)",
    data_files=["data/wav_integrated_waves.json"])
E = dict(engine=5)
cases = [("h0 t0 m0", dict(harmonics=0, timbre=0, morph=0)),
         ("h.5 t.5 m.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
         ("h1 t1 m1", dict(harmonics=1, timbre=1, morph=1)),
         ("h.2 t.8 m.2 (T1/T2)", dict(harmonics=0.2, timbre=0.8, morph=0.2)),
         ("h.45 t1 m.4 (T3/T4)", dict(harmonics=0.45, timbre=1.0, morph=0.4)),
         ("h.75 t.7 m.6 (WT banks)", dict(harmonics=0.75, timbre=0.7, morph=0.6)),
         ("h.9 t.9 m0 n72", dict(harmonics=0.9, timbre=0.9, morph=0.0, note=72)),
         ("h.3 t.6 m.9 n30", dict(harmonics=0.3, timbre=0.6, morph=0.9, note=30))]
for lab, kw in cases:
    t.compare(lab, **E, **kw)
t.compare("sweep", seconds=4, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1, **E)
t.compare("trig gate .5 retrig .25", gate=0.1, retrig=0.25, harmonics=0.6, timbre=0.5, morph=0.5, **E)
if "cpu" in sys.argv: t.cpu(harmonics=0.6)
