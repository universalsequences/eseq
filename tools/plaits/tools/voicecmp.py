"""Compare the assembled instrument against plref mode=voice."""
import os, subprocess, sys
import numpy as np
sys.path.insert(0, os.path.dirname(__file__))
import plh  # sets env
A = plh.A
ROOT = plh.ROOT
DELAY = 48

class VoiceTest:
    def __init__(self, only=None, tag="voice"):
        self.dir = os.path.join(plh.WORK, "tests", tag)
        cmd = [sys.executable, os.path.join(ROOT, "tools", "assemble.py"), self.dir, "--ref-tune"]
        if only:
            cmd += ["--only", ",".join(only)]
        subprocess.run(cmd, check=True)
        self.inst = A.Instrument(self.dir, max_frames=12)

    def ref(self, **kw):
        path = os.path.join(self.dir, "ref.raw")
        subprocess.run([plh.REF, path, "mode=voice"] + [f"{k}={v}" for k, v in kw.items()], check=True)
        return np.fromfile(path, dtype=np.float32).reshape(-1, 2)

    def compare(self, label, model, lpg_mode=0, vel=1.0, port=None, wav=None, **kw):
        """kw: plref voice keys (engine set from model). Port: pitch from note."""
        kw.setdefault("seconds", 2.0)
        pidx = (model + 8) % 24
        ref_kw = dict(kw, engine=pidx, level_patched=lpg_mode, level=vel, level_delay=DELAY)
        r = self.ref(**ref_kw)
        note = kw.get("note", 48)
        params = dict(model=model, lpg_mode=lpg_mode, level_db=0,
                      harmonics=kw.get("harmonics", 0.5), timbre=kw.get("timbre", 0.5),
                      morph=kw.get("morph", 0.5), decay=kw.get("decay", 0.5), lpg=kw.get("lpg", 0.5),
                      fm_amt=kw.get("fm_amt", 0), timbre_amt=kw.get("timbre_amt", 0),
                      morph_amt=kw.get("morph_amt", 0))
        params.update(port or {})
        hz = 440 * 2 ** ((note - 69) / 12)
        # The module delays its trigger by 5 blocks (60 samples): drive the
        # port's gate with the same delay (and no t=0 trigger pulse).
        sr = 48000.0
        gs, rt, secs = kw.get("gate", 1.0), kw.get("retrig", 0), kw["seconds"]
        g = []
        for b in range(0, int(secs * sr), 12):
            t = b / sr
            if rt > 0:
                t = t % rt
            g.append(1.0 if (gs >= 0 and t < gs) else 0.0)
        pts = [(0.0, 0.0)]
        prev = 0.0
        for i, v in enumerate(g):
            if v != prev:
                ts = (i * 12 + DELAY) / sr
                pts += [(ts - 1e-9, prev), (ts, v)]
                prev = v
        y, _ = self.inst.render(secs, pitch=hz, vel=vel, params=params,
                                ramps={"gate": pts, "trigger": [(0.0, 0.0)]})
        p = np.asarray(y)
        mix = params.get("aux_mix", 0)
        rr = r[:, 0] * (1 - mix) + r[:, 1] * mix
        pp = p[:, 0]
        n = min(len(rr), len(pp))
        rr, pp = rr[:n], pp[:n]
        best = min(range(-14, 15), key=lambda L: np.mean((np.roll(pp, L) - rr) ** 2))
        e = np.roll(pp, best) - rr
        rms = np.sqrt(np.mean(rr ** 2)); prms = np.sqrt(np.mean(pp ** 2))
        e0 = slice(0, 4800)
        print(f"{label:28s} ref {rms:.4f} port {prms:.4f} lag {best:+d} err {np.sqrt(np.mean(e**2))/max(rms,1e-9):.4f} "
              f"err100ms {np.sqrt(np.mean(e[e0]**2))/max(np.sqrt(np.mean(rr[e0]**2)),1e-9):.4f}", flush=True)
        return rr, pp
