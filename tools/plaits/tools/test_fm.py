import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("fm", ["engines/fm.lisp"],
    "(plaits-fm bstart bphase rise high patched note timbre morph harmonics accent)",
    data_files=["data/lut_fm_frequency_quantizer.json"])
for h, tb, m in [(0.5, 0.5, 0.5), (0.2, 0.8, 0.2), (0.8, 0.3, 0.9), (0.6, 1.0, 0.0)]:
    t.compare(f"h{h} t{tb} m{m}", engine=10, note=48, harmonics=h, timbre=tb, morph=m)
t.compare("sweep", engine=10, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0.5, seconds=3)
