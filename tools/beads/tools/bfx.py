"""Beads effect harness: compiles content/effects/Beads with the pinned
DGenLisp (via tools/audition) and renders it over a stereo input, and runs
the original Processor (tools/beads/ref) on the same input."""
import ctypes, os, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, os.path.join(REPO, "tools", "audition"))
os.environ.setdefault("ESEQ_DGENLISP_TOOL", os.path.join(REPO, "crates", "sequencer", "tools", "DGenLisp-macos-arm64"))
os.environ.setdefault("ESEQ_DGEN_TOOLCHAIN_ROOT", os.path.join(REPO, "crates", "sequencer", "tools", "dgen-toolchain"))
import audition as A  # noqa: E402

EFFECT = os.environ.get("BEADS_FX", os.path.join(REPO, "content", "effects", "Beads"))
WORK = os.environ.get("BEADS_WORK", os.path.expanduser("~/.cache/eseq-beads"))
REF = os.environ.get("BEADS_REF", os.path.join(WORK, "bfxref"))
SR = 48000

# Param order of tools/beads/ref/bfxref.cc after `media`.
REF_KEYS = ["time", "time_amt", "size", "size_amt", "shape", "shape_amt", "pitch",
            "pitch_amt", "density", "feedback", "dry_wet", "reverb", "freeze", "seed"]


class Effect(A.Instrument):
    def render_fx(self, x, params=None, ramps=None, mem=None, exact=False):
        """x: (n, 2) float input. ramps: {param: [(t, v), ...]} step changes,
        applied per host block (exact=True: per 12-frame block, so they land
        where the reference's do)."""
        n = len(x)
        blk = 12 if exact and ramps else self.max_frames
        if mem is None:
            mem = self.fresh_memory()
        for name, val in (params or {}).items():
            mem[self.params[name]["cellId"]] = val
        ins = [np.zeros(blk, dtype=np.float32) for _ in range(self.n_in)]
        outs = [np.zeros(blk, dtype=np.float32) for _ in range(self.n_out)]
        P = ctypes.POINTER(ctypes.c_float)
        inptrs = (P * self.n_in)(*[a.ctypes.data_as(P) for a in ins])
        outptrs = (P * self.n_out)(*[a.ctypes.data_as(P) for a in outs])
        li, ri = self.inputs["left"], self.inputs["right"]
        y = np.zeros((n, 2), dtype=np.float32)
        for b in range(0, n, blk):
            fr = min(blk, n - b)
            ins[li][:] = 0; ins[ri][:] = 0
            ins[li][:fr] = x[b:b + fr, 0]
            ins[ri][:fr] = x[b:b + fr, 1]
            for name, pts in (ramps or {}).items():
                t = b / self.sample_rate
                v = pts[0][1]
                for (t0, v0) in pts:
                    if t >= t0:
                        v = v0
                mem[self.params[name]["cellId"]] = v
            self.process_fn(inptrs, outptrs, fr, mem.ctypes.data_as(ctypes.c_void_p),
                            ctypes.byref(self.context), None)
            y[b:b + fr, 0] = outs[0][:fr]
            y[b:b + fr, 1] = outs[1][:fr]
        return y


def effect(path=EFFECT, **kw):
    return Effect(path, sample_rate=SR, **kw)


def ref(x, media=3, seed_at=None, freeze_at=None, **params):
    """Original Processor over x; params by REF_KEYS name (seed: 0 off, 1 held, 2 locked)."""
    os.makedirs(WORK, exist_ok=True)
    pin, pout = os.path.join(WORK, "in.raw"), os.path.join(WORK, "out.raw")
    np.asarray(x, dtype=np.float32).tofile(pin)
    args = [REF, pin, pout, str(media)] + [str(params[k]) for k in REF_KEYS]
    env = dict(os.environ)
    if seed_at:
        env["SEED_AT"] = ",".join(str(t) for t in seed_at)
    if freeze_at:
        env["FREEZE_AT"] = ",".join(str(t) for t in freeze_at)
    subprocess.run(args, check=True, env=env)
    return np.fromfile(pout, dtype=np.float32).reshape(-1, 2)


def test_signal(seconds=3.0, seed=1):
    """A melodic stereo test input: plucked partials + a little noise."""
    rng = np.random.default_rng(seed)
    n = int(seconds * SR)
    t = np.arange(n) / SR
    y = np.zeros((n, 2), dtype=np.float32)
    for k, start in enumerate(np.arange(0, seconds, 0.25)):
        f = 110 * 2 ** (rng.integers(0, 24) / 12)
        s = int(start * SR)
        tt = t[s:] - start
        env = np.exp(-tt * 6)
        v = 0.3 * env * (np.sin(2 * np.pi * f * tt) + 0.3 * np.sin(2 * np.pi * 2.01 * f * tt))
        y[s:, k % 2] += v
        y[s:, 1 - k % 2] += 0.5 * v
    y += 0.01 * rng.standard_normal((n, 2))
    return y.astype(np.float32)
