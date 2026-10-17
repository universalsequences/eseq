import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh
t = plh.EngineTest("chip", ["engines/chip.lisp"],
    "(plaits-chip bstart bphase rise high patched note timbre morph harmonics accent p_env)",
    extra_params=["(param p_env @default 2 @min -2 @max 2)"])
E = dict(engine=7)
def cmp(label, env=2, **kw):
    return t.compare(label, port={"p_env": env}, env_shape=env, **E, **kw)
# clocked: arpeggiator, clock every 0.125 s
for lab, kw in [("arp up x1 m7 h.4 t0 m0", dict(harmonics=0.4, timbre=0.0, morph=0.0)),
                ("arp up x4 M9 h.75 t.2 m.5", dict(harmonics=0.75, timbre=0.2, morph=0.5)),
                ("arp down x2 m9 h.5 t.35 m1", dict(harmonics=0.5, timbre=0.35, morph=1.0)),
                ("arp updown x4 h.6 t.7 m.3", dict(harmonics=0.6, timbre=0.7, morph=0.3)),
                ("arp updown x2 oct h0 t.6", dict(harmonics=0.0, timbre=0.6, morph=0.7)),
                ("arp n60 h1 t.5 m.8", dict(harmonics=1.0, timbre=0.5, morph=0.8, note=60))]:
    cmp(lab, gate=0.05, retrig=0.125, seconds=3, **kw)
cmp("arp sweep", gate=0.05, retrig=0.125, seconds=4, note=36, note_end=72, harmonics=0, harmonics_end=1, timbre=0, timbre_end=0.74, morph=0, morph_end=1)
cmp("env .3 updown", env=0.3, gate=0.05, retrig=0.25, harmonics=0.6, timbre=0.7, morph=0.3)
cmp("env -.6 up", env=-0.6, gate=0.05, retrig=0.25, harmonics=0.4, timbre=0.1, morph=0.6)
cmp("env .02 gate.5", env=0.02, gate=0.5, harmonics=0.4, timbre=0.1, morph=0.6)
cmp("random arp (stats)", gate=0.05, retrig=0.125, seconds=3, harmonics=0.6, timbre=0.95, morph=0.4)
# unclocked: chord inversions
for lab, kw in [("chord h0 t0 m0", dict(harmonics=0, timbre=0, morph=0)),
                ("chord h.5 t.5 m.5", dict(harmonics=0.5, timbre=0.5, morph=0.5)),
                ("chord h1 t1 m1", dict(harmonics=1, timbre=1, morph=1)),
                ("chord h.3 t.83 m.2 n60", dict(harmonics=0.3, timbre=0.83, morph=0.2, note=60))]:
    cmp(lab, trig=0, **kw)
cmp("chord sweep", trig=0, seconds=4, note=36, note_end=72, harmonics=0, harmonics_end=1, timbre=0, timbre_end=1, morph=0, morph_end=1)
if "cpu" in sys.argv: t.cpu(trig=0)
