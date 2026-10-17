import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("va", ["engines/va.lisp"],
    "(plaits-va bstart bphase rise high patched note timbre morph harmonics accent)")
E = 8
for h, tb, m in [(0.5, 0.5, 0.5), (0.0, 0.0, 0.0), (1.0, 1.0, 1.0), (0.2, 0.8, 0.3), (0.8, 0.3, 0.9), (0.0, 1.0, 0.6), (0.65, 0.7, 0.1)]:
    t.compare(f"h{h} t{tb} m{m}", engine=E, note=48, harmonics=h, timbre=tb, morph=m)
t.compare("high note 84", engine=E, note=84, harmonics=0.3, timbre=0.6, morph=0.7)
t.compare("note 100 clamp", engine=E, note=100, harmonics=0.9, timbre=1.0, morph=0.2)
t.compare("sweep", engine=E, note=36, note_end=96, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=1, morph_end=0, seconds=3)
t.compare("gate/retrig", engine=E, note=60, gate=0.5, retrig=0.25, harmonics=0.3, timbre=0.6, morph=0.4)
t.compare("unpatched", engine=E, note=60, trig=0, harmonics=0.3, timbre=0.6, morph=0.4)
if "cpu" in sys.argv: t.cpu(note=48)
