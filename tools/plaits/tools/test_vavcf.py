import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("vavcf", ["engines/vavcf.lisp"],
    "(plaits-vavcf bstart bphase rise high patched note timbre morph harmonics accent)")
for h, tb, m in [(0.5, 0.5, 0.5), (0.0, 0.3, 0.0), (1.0, 0.8, 1.0), (0.2, 1.0, 0.8), (0.9, 0.0, 0.3), (0.65, 0.6, 0.7)]:
    t.compare(f"h{h} t{tb} m{m}", engine=0, note=48, harmonics=h, timbre=tb, morph=m)
t.compare("sweep note/h/t", engine=0, note=24, note_end=96, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0.5, seconds=3)
t.compare("sweep morph", engine=0, note=60, morph=0, morph_end=1, harmonics=0.3, timbre=0.6, seconds=3)
t.compare("gate retrig", engine=0, note=40, gate=0.5, retrig=0.25, harmonics=0.7)
t.cpu()
