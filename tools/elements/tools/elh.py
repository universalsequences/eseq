#!/usr/bin/env python3
"""Elements port test harness: renders the assembled test driver
(tools/assemble.py --test) at 32 kHz and compares it with ref/elref.

    import elh
    t = elh.Test()
    t.compare("label", seconds=1, note=48, strike=0.8, ...)

Keys (both sides): seconds gate retrig note note_end strength model contour
bow bow_timbre blow blow_meta blow_timbre strike strike_meta strike_timbre
signature geometry(+_end) brightness(+_end) damping(+_end) position(+_end)
space rev_diffusion rev_lp; det=1 + det_d/det_strike/det_str for the
deterministic noise layout.
Channels: main aux raw center sides bow blow strike env.
"""
import os, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
REF = os.path.join(ROOT, "ref", "elref")
REPO = "/Users/alecresende/code/learning/anthropic/eseq"
sys.path.insert(0, os.path.join(REPO, "tools", "audition"))
os.environ.setdefault("ESEQ_DGENLISP_TOOL", REPO + "/crates/sequencer/tools/DGenLisp-macos-arm64")
os.environ.setdefault("ESEQ_DGEN_TOOLCHAIN_ROOT", REPO + "/crates/sequencer/tools/dgen-toolchain")
import audition as A  # noqa: E402

WORK = os.environ.get("ELEMENTS_WORK", os.path.expanduser("~/.cache/eseq-elements"))
INSTRUMENT = os.path.join(REPO, "content", "instruments", "Synths", "Elements")
DATA = os.environ.get("ELEMENTS_DATA", os.path.join(INSTRUMENT, "data"))


def link_data(dst, src=DATA):
    """Symlink src's .json data files into dst (replacing dangling links)."""
    for f in os.listdir(dst):
        p = os.path.join(dst, f)
        if os.path.islink(p) and not os.path.exists(p):
            os.unlink(p)
    for f in os.listdir(src):
        if f.endswith(".json") and not os.path.exists(os.path.join(dst, f)):
            os.symlink(os.path.join(src, f), os.path.join(dst, f))
CH = ["main", "aux", "raw", "center", "sides", "bow", "blow", "strike", "env"]
PATCH = ["contour", "bow", "bow_timbre", "blow", "blow_meta", "blow_timbre", "strike", "strike_meta",
         "strike_timbre", "signature", "geometry", "brightness", "damping", "position", "space",
         "rev_diffusion", "rev_lp"]


class Test:
    def __init__(self, name="driver", sample_rate=32000.0):
        self.dir = os.path.join(WORK, name)
        subprocess.run([sys.executable, os.path.join(HERE, "assemble.py"), self.dir, "--test"], check=True)
        self.inst = A.Instrument(self.dir, sample_rate=sample_rate)

    def ref(self, **kw):
        path = os.path.join(self.dir, "ref.raw")
        env = dict(os.environ)
        if kw.get("det"):
            env["ELREF_DET"] = "1"
        args = [REF, path] + [f"{k}={v}" for k, v in kw.items() if not k.startswith("det")]
        subprocess.run(args, check=True, env=env)
        return np.fromfile(path, dtype=np.float32).reshape(-1, 9)

    def port(self, **kw):
        m = {"seconds": "seconds", "gate": "gate_s", "retrig": "retrig", "note": "t_note",
             "note_end": "t_note_end", "strength": "t_strength", "model": "t_model",
             "det": "det", "det_d": "det_d", "det_strike": "det_strike", "det_str": "det_str"}
        for p in PATCH:
            m[p] = "t_" + p
            m[p + "_end"] = "t_" + p + "_end"
        params = {m[k]: float(v) for k, v in kw.items() if k in m}
        if kw.get("easter"):
            params["t_model"] = 3.0
        y, _ = self.inst.render(float(kw.get("seconds", 2.0)), pitch=440.0, vel=1.0, params=params)
        return np.asarray(y, dtype=np.float32)[:, :9]

    def compare(self, label="", chans=("main", "aux"), maxlag=2, **kw):
        kw.setdefault("seconds", 1.0)
        r = self.ref(**kw)
        p = self.port(**kw)
        n = min(len(r), len(p))
        r, p = r[:n], p[:n]
        e0 = slice(0, min(n, 3200))
        res = {}
        parts = []
        for nm in chans:
            ch = CH.index(nm)
            rr = float(np.sqrt(np.mean(r[:, ch] ** 2)))
            pr = float(np.sqrt(np.mean(p[:, ch] ** 2)))
            best = min(range(-maxlag, maxlag + 1),
                       key=lambda L: np.mean((np.roll(p[:, ch], L) - r[:, ch]) ** 2))
            err = float(np.sqrt(np.mean((np.roll(p[:, ch], best) - r[:, ch]) ** 2)))
            rr0 = float(np.sqrt(np.mean(r[e0, ch] ** 2)))
            err0 = float(np.sqrt(np.mean((np.roll(p[:, ch], best)[e0] - r[e0, ch]) ** 2)))
            res[nm] = (rr, pr, best, err / max(rr, 1e-9), err0 / max(rr0, 1e-9))
            parts.append(f"{nm}: ref {rr:.4f} port {pr:.4f} lag {best:+d} err {err / max(rr, 1e-9):.4f} e100 {err0 / max(rr0, 1e-9):.4f}")
        print(f"{label:22s} " + "  ".join(parts), flush=True)
        return r, p, res
