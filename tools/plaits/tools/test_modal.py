import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh_pm
t = plh_pm.PMTest("modal", ["engines/modal.lisp"],
    "(plaits-modal bstart bphase rise high patched note timbre morph harmonics accent)",
    data_files=["data/lut_stiffness.json"])
quick = len(sys.argv) > 1 and sys.argv[1] == "quick"
E = 20
# Triggered: the exciter is a deterministic impulse -> exact comparison.
t.compare("h0.5 t0.5 m0.5", engine=E, note=48, harmonics=0.5, timbre=0.5, morph=0.5)
if quick: sys.exit()
t.compare("h0 t0 m0", engine=E, note=48, harmonics=0.0, timbre=0.0, morph=0.0)
t.compare("h1 t1 m1", engine=E, note=48, harmonics=1.0, timbre=1.0, morph=1.0)
t.compare("h0.2 t0.8 m0.3", engine=E, note=60, harmonics=0.2, timbre=0.8, morph=0.3)
t.compare("h0.8 t0.3 m0.9 n36", engine=E, note=36, harmonics=0.8, timbre=0.3, morph=0.9)
t.compare("high note n96", engine=E, note=96, harmonics=0.6, timbre=0.7, morph=0.6)
t.compare("retrig", engine=E, note=55, harmonics=0.4, timbre=0.6, morph=0.7, gate=0.05, retrig=0.25)
t.compare("sweep retrig", engine=E, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0.2, morph_end=0.9, gate=0.05, retrig=0.25, seconds=3)
# Unpatched (dust) with deterministic noise (Random::GetFloat() == u).
t.compare("unpatched u.001", u=0.001, engine=E, trig=0, note=48, harmonics=0.4, timbre=0.5, morph=0.6)
t.compare("unpatched u.5 t1", u=0.5, engine=E, trig=0, note=60, harmonics=0.7, timbre=0.95, morph=0.4)
# Unpatched with random dust: statistics.
t.stats("rand unpatched t.5", engine=E, trig=0, note=48, harmonics=0.4, timbre=0.5, morph=0.6)
t.stats("rand unpatched t.9", engine=E, trig=0, note=60, harmonics=0.7, timbre=0.9, morph=0.4)
t.cpu(seconds=5, gate=0.05, retrig=0.25)
