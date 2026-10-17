"""plh variant for the noise-excited physical-modelling engines (string, modal).

Adds a deterministic-noise mode: compare(u=0.3, ...) renders the C++ engine
with tools/plref_u/plref_u (stmlib Random::GetFloat() returns the constant
PLREF_U) and the port with u0 = the same constant (param p_u), so the whole
exciter + resonator chain can be compared sample for sample. Without `u`,
both sides use their own random noise; use stats() to compare statistics.
"""
import os
import subprocess
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plh  # noqa: E402

plh.DRIVER_BODY = plh.DRIVER_BODY.replace(
    "(def u0 (noise))", "(def u0 (gswitch (gte p_u 0) p_u (noise)))")
REF_U = os.path.join(plh.ROOT, "tools", "plref_u", "plref_u")
if not os.path.exists(REF_U):
    subprocess.run(["bash", os.path.join(plh.ROOT, "tools", "plref_u", "build.sh")], check=True)


class PMTest(plh.EngineTest):
    def __init__(self, name, lisp_files, call, data_files=(), extra_params=(), **kw):
        super().__init__(name, lisp_files, call, data_files,
                         list(extra_params) + ["(param p_u @default -1 @min -1 @max 1)"], **kw)
        self._u = None

    def ref(self, **kw):
        path = os.path.join(self.dir, "ref.raw")
        env = dict(os.environ)
        exe = plh.REF
        if self._u is not None:
            exe = REF_U
            env["PLREF_U"] = repr(float(self._u))
        args = [exe, path, "mode=engine"] + [f"{k}={v}" for k, v in kw.items()]
        subprocess.run(args, check=True, env=env)
        return np.fromfile(path, dtype=np.float32).reshape(-1, 2)

    def compare(self, label="", port=None, u=None, **kw):
        self._u = u
        port = dict(port or {})
        if u is not None:
            port["p_u"] = float(u)
        try:
            return super().compare(label, port=port, **kw)
        finally:
            self._u = None

    def stats(self, label="", win=0.1, runs=8, seeds=3, port=None, **kw):
        """Random noise on both sides: one long render (runs x seconds) split
        into `runs` windows on each side (the reference repeated for
        PLREF_SEED=1..seeds and pooled); sweeps are not supported here. Reports mean rms
        +- sd across runs, the mean |dB| distance between the run-averaged rms
        envelopes (win-second windows), and the mean spectral centroid."""
        sec = float(kw.pop("seconds", 2.0))
        n = int(sec * 48000) // 12 * 12
        refs = []
        for seed in range(1, seeds + 1):
            path = os.path.join(self.dir, "ref.raw")
            env = dict(os.environ, PLREF_SEED=str(seed))
            args = [REF_U, path, "mode=engine", f"seconds={sec * runs}"] + [f"{k}={v}" for k, v in kw.items()]
            subprocess.run(args, check=True, env=env)
            r = np.fromfile(path, dtype=np.float32).reshape(-1, 2)
            refs += [r[i * n:(i + 1) * n] for i in range(runs)]
        long = self.port(port=port, seconds=sec * runs, **kw)
        ports = [long[i * n:(i + 1) * n] for i in range(runs)]
        W = int(win * 48000)

        def env(x):
            k = len(x) // W
            return np.sqrt(np.mean(x[:k * W].reshape(k, W) ** 2, axis=1))

        def centroid(x):
            X = np.abs(np.fft.rfft(x * np.hanning(len(x))))
            f = np.fft.rfftfreq(len(x), 1 / 48000)
            return float(np.sum(f * X) / max(np.sum(X), 1e-12))
        out = []
        for ch, nm in ((0, "out"), (1, "aux")):
            rr = np.array([np.sqrt(np.mean(x[:, ch] ** 2)) for x in refs])
            pr = np.array([np.sqrt(np.mean(x[:, ch] ** 2)) for x in ports])
            er = np.sqrt(np.mean([env(x[:, ch]) ** 2 for x in refs], axis=0))
            ep = np.sqrt(np.mean([env(x[:, ch]) ** 2 for x in ports], axis=0))
            m = (er > 1e-5) | (ep > 1e-5)
            edb = float(np.mean(np.abs(20 * np.log10((ep[m] + 1e-9) / (er[m] + 1e-9))))) if m.any() else 0.0
            cr = np.mean([centroid(x[:, ch]) for x in refs])
            cp = np.mean([centroid(x[:, ch]) for x in ports])
            out.append((nm, rr.mean(), rr.std(), pr.mean(), pr.std(), edb, cr, cp))
        line = "  ".join(f"{nm}: rms ref {a:.4f}+-{b:.4f} port {c:.4f}+-{d:.4f} envdB {e:.2f} centroid {cr:.0f}/{cp:.0f}Hz"
                         for nm, a, b, c, d, e, cr, cp in out)
        print(f"{label:24s} {line}", flush=True)
        return refs, ports, out
