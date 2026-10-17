"""Compile a text-patched copy of the test driver: variant(name, [(old, new), ...])."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh
def variant(name, subs, sample_rate=32000.0):
    base = os.path.join(elh.WORK, "driver")
    src = open(os.path.join(base, "dsp.lisp")).read()
    for a, b in subs:
        assert a in src, a
        src = src.replace(a, b)
    d = os.path.join(elh.WORK, "var_" + name)
    os.makedirs(d, exist_ok=True)
    elh.link_data(d, base)
    open(os.path.join(d, "dsp.lisp"), "w").write(src)
    t = elh.Test.__new__(elh.Test)
    t.dir = d
    t.inst = elh.A.Instrument(d, sample_rate=sample_rate)
    return t
