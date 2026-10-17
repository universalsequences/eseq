import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("chord", ["engines/chordbank.lisp", "engines/chord.lisp"],
    "(plaits-chord bstart bphase rise high patched note timbre morph harmonics accent)",
    data_files=["data/wav_integrated_waves.json"])
E = 14
for h, tb, m in [(0.0, 0.0, 0.0), (0.3, 0.5, 0.2), (0.5, 0.25, 0.45), (0.65, 0.8, 0.5), (0.85, 0.6, 0.75), (1.0, 1.0, 1.0)]:
    t.compare(f"h{h} t{tb} m{m}", engine=E, note=48, harmonics=h, timbre=tb, morph=m)
t.compare("note80 h0.4", engine=E, note=80, harmonics=0.4, timbre=0.4, morph=0.3)
t.compare("trig g.5 r.25", engine=E, note=45, harmonics=0.7, timbre=0.3, morph=0.6, gate=0.5, retrig=0.25)
t.compare("sweep", engine=E, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1, seconds=3)
# endpoints avoid exact ties (timbre_lp*20 == 1.0 at a block boundary flips one block early on 1-ulp sweep differences)
t.compare("sweep2", engine=E, note=60, harmonics=0.97, harmonics_end=0.013, timbre=0.983, timbre_end=0.007, morph=0.991, morph_end=0.011, seconds=3)
t.compare("trig=0 unpatched", engine=E, note=52, harmonics=0.2, timbre=0.55, morph=0.52, trig=0)
if "cpu" in sys.argv: t.cpu()
