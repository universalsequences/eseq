import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("wavetable", ["engines/wavetable.lisp"],
    "(plaits-wavetable bstart bphase rise high patched note timbre morph harmonics accent)",
    data_files=["data/wav_integrated_waves.json"])
E = 13
for h, tb, m in [(0.0, 0.0, 0.0), (0.2, 0.5, 0.5), (0.4, 0.8, 0.2), (0.55, 0.3, 0.9), (0.7, 1.0, 1.0), (1.0, 0.6, 0.4)]:
    t.compare(f"h{h} t{tb} m{m}", engine=E, note=48, harmonics=h, timbre=tb, morph=m)
t.compare("note72 h0.9", engine=E, note=72, harmonics=0.9, timbre=0.35, morph=0.65)
t.compare("trig g.5 r.25", engine=E, note=40, harmonics=0.3, timbre=0.7, morph=0.3, gate=0.5, retrig=0.25)
t.compare("sweep", engine=E, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=1, morph_end=0, seconds=3)
if "cpu" in sys.argv: t.cpu()
