import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh_pm
t = plh_pm.PMTest("string", ["engines/string.lisp"],
    "(plaits-string bstart bphase rise high patched note timbre morph harmonics accent p_active)",
    data_files=["data/lut_svf_shift.json"],
    extra_params=["(param p_active @default 1 @min 0 @max 1)"])
quick = len(sys.argv) > 1 and sys.argv[1] == "quick"
E = 19
# Deterministic exciter (Random::GetFloat() == u on both sides).
t.compare("curved h0.1 u.3", u=0.3, engine=E, note=48, harmonics=0.1, timbre=0.5, morph=0.5)
if quick: sys.exit()
t.compare("plain h0.25 u.8", u=0.8, engine=E, note=60, harmonics=0.25, timbre=0.7, morph=0.7)
t.compare("disp h0.6 u.3", u=0.3, engine=E, note=48, harmonics=0.6, timbre=0.3, morph=0.6)
t.compare("disp h1 t1 m1 u.2", u=0.2, engine=E, note=52, harmonics=1.0, timbre=1.0, morph=1.0)
t.compare("curved h0 t0 m0 u.9", u=0.9, engine=E, note=40, harmonics=0.0, timbre=0.0, morph=0.0)
t.compare("m0.97 inf decay", u=0.3, engine=E, note=55, harmonics=0.4, timbre=0.6, morph=0.97)
t.compare("high note n100", u=0.7, engine=E, note=100, harmonics=0.05, timbre=0.8, morph=0.8)
t.compare("low note SRC", u=0.3, engine=E, note=24, harmonics=0.3, timbre=0.5, morph=0.7)
t.compare("retrig 3 strings", u=0.3, engine=E, note=50, harmonics=0.5, timbre=0.6, morph=0.8, gate=0.05, retrig=0.25)
t.compare("sweep retrig", u=0.4, engine=E, note=36, note_end=84, harmonics=0, harmonics_end=1, timbre=0.2, timbre_end=1, morph=0.3, morph_end=0.9, gate=0.05, retrig=0.25, seconds=3)
t.compare("unpatched dust u.001", u=0.001, engine=E, trig=0, note=48, harmonics=0.4, timbre=0.5, morph=0.6)
t.compare("unpatched dust u.5 t1", u=0.5, engine=E, trig=0, note=48, harmonics=0.7, timbre=0.95, morph=0.6)
# Real random noise on both sides: statistics only.
t.stats("rand pluck retrig", engine=E, note=48, harmonics=0.5, timbre=0.5, morph=0.6, gate=0.05, retrig=0.25)
t.stats("rand disp noise h1", engine=E, note=48, harmonics=1.0, timbre=0.7, morph=0.8, gate=0.05, retrig=0.5)
t.stats("rand unpatched t.6", engine=E, trig=0, note=48, harmonics=0.3, timbre=0.6, morph=0.7)
t.stats("rand unpatched t.9", engine=E, trig=0, note=60, harmonics=0.8, timbre=0.9, morph=0.5)
print("active=1:", end=" "); t.cpu(seconds=5, gate=0.05, retrig=0.25, harmonics=0.6)
print("active=0:", end=" "); t.cpu(seconds=5, gate=0.05, retrig=0.25, harmonics=0.6, port={"p_active": 0})
# Deselect mid-note then reselect: silent while inactive, lines cleared.
import numpy as np
y = t.port(seconds=1, gate=0.05, retrig=0.25, note=48, harmonics=0.5, port={"p_active": 0})
print("inactive output peak:", float(np.abs(y).max()))
