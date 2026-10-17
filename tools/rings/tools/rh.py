#!/usr/bin/env python3
"""Rings port test harness: assembles the instrument (tools/assemble.py)
and compares it with the C++ reference (ref/rref) at 48 kHz.

    import rh
    t = rh.VoiceTest()                 # or rh.VoiceTest(const_u=0.73)
    t.compare("modal", model=0, note=48, structure=0.3)
"""
import os, subprocess, sys, time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)                      # tools/rings
REPO = os.path.dirname(os.path.dirname(ROOT))
REF = os.path.join(ROOT, "ref", "rref")
WORK = os.environ.get("RINGS_WORK", os.path.expanduser("~/.cache/eseq-rings"))
sys.path.insert(0, os.path.join(REPO, "tools", "audition"))
os.environ.setdefault("ESEQ_DGENLISP_TOOL", REPO + "/crates/sequencer/tools/DGenLisp-macos-arm64")
os.environ.setdefault("ESEQ_DGEN_TOOLCHAIN_ROOT", REPO + "/crates/sequencer/tools/dgen-toolchain")
import audition as A  # noqa: E402

PATCH = ("structure", "brightness", "damping", "position")


class VoiceTest:
    def __init__(self, const_u=None, tag=None, sample_rate=48000.0, noise_out=False):
        tag = tag or ("voice" if const_u is None else f"voice_u{const_u}") + ("_n" if noise_out else "")
        self.const_u = const_u
        self.noise_out = noise_out
        self.dir = os.path.join(WORK, "tests", tag)
        cmd = [sys.executable, os.path.join(HERE, "assemble.py"), self.dir]
        if const_u is not None:
            cmd += ["--const-u", str(const_u)]
        if noise_out:
            cmd += ["--noise-out"]
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)
        self.inst = A.Instrument(self.dir, max_frames=24, sample_rate=sample_rate)

    def ref(self, noise=None, **kw):
        path = os.path.join(self.dir, "ref.raw")
        env = dict(os.environ)
        if self.const_u is not None:
            env["RREF_U"] = str(self.const_u)
        if noise is not None:
            npath = os.path.join(self.dir, "noise.raw")
            np.asarray(noise, dtype=np.float32).tofile(npath)
            env["RREF_NOISE"] = npath
        subprocess.run([REF, path] + [f"{k}={v}" for k, v in kw.items()], check=True, env=env)
        return np.fromfile(path, dtype=np.float32).reshape(-1, 2)

    def port(self, seconds=2.0, note=48, retrig=0, model=0, **kw):
        params = dict(model=model, level_db=0, spread=1, tune=0)
        params.update({k: kw[k] for k in PATCH if k in kw})
        ramps = {}
        for k in PATCH + ("note",):
            if k + "_end" in kw:
                a = kw.get(k, note if k == "note" else 0.5)
                b = kw[k + "_end"]
                if k == "note":
                    # one point per 24-sample block: the ref sweeps the note linearly
                    ts = np.arange(0, seconds, 24 / 48000)
                    ramps["pitch"] = [(t, 440 * 2 ** ((a + (b - a) * t / seconds - 69) / 12)) for t in ts]
                else:
                    ramps[k] = [(0, a), (seconds, b)]
        retrigs = [i * retrig for i in range(1, int(seconds / retrig) + 1)] if retrig else None
        hz = 440 * 2 ** ((note - 69) / 12)
        y, _ = self.inst.render(seconds, pitch=hz, vel=1.0, params=params, retrig=retrigs, ramps=ramps or None)
        return np.asarray(y, dtype=np.float32)

    def compare(self, label, maxlag=4, wav=None, quiet=False, **kw):
        kw.setdefault("seconds", 2.0)
        p = self.port(**kw)
        noise = None
        if self.noise_out:
            # the port's own noise streams drive the reference
            noise, p = p[:, 2:4], p[:, :2]
        r = self.ref(noise=noise, **kw)
        n = min(len(r), len(p))
        r, p = r[:n], p[:n]
        res = []
        for ch, nm in ((0, "odd"), (1, "even")):
            rr = float(np.sqrt(np.mean(r[:, ch] ** 2)))
            pr = float(np.sqrt(np.mean(p[:, ch] ** 2)))
            mse = {L: np.mean((np.roll(p[:, ch], L)[64:-64] - r[64:-64, ch]) ** 2)
                   for L in range(-maxlag, maxlag + 1)}
            best = min(mse, key=mse.get)
            if mse[0] <= mse[best] * 1.01 + 1e-20:
                best = 0
            err = float(np.sqrt(np.mean((np.roll(p[:, ch], best)[64:-64] - r[64:-64, ch]) ** 2)))
            e0 = slice(0, 4800)
            rr0 = float(np.sqrt(np.mean(r[e0, ch] ** 2)))
            err0 = float(np.sqrt(np.mean((np.roll(p[:, ch], best)[e0] - r[e0, ch]) ** 2)))
            res.append((nm, rr, pr, best, err / max(rr, 1e-9), err0 / max(rr0, 1e-9)))
        if not quiet:
            line = "  ".join(f"{nm}: ref {rr:.4f} port {pr:.4f} lag {lag:+d} err {e:.4f} e100 {e0:.4f}"
                             for nm, rr, pr, lag, e, e0 in res)
            print(f"{label:26s} {line}", flush=True)
        if wav:
            import wave
            for tag, y in (("ref", r), ("port", p)):
                with wave.open(f"{wav}-{tag}.wav", "wb") as w:
                    w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
                    w.writeframes((np.clip(y, -1, 1) * 32767).astype("<i2").tobytes())
        return r, p, res

    def cpu(self, seconds=10.0, **kw):
        """Fraction of one core in real time (48 kHz)."""
        self.port(seconds=0.1, **kw)
        t0 = time.perf_counter()
        self.port(seconds=seconds, **kw)
        return (time.perf_counter() - t0) / seconds
