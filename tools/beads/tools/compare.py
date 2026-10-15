#!/usr/bin/env python3
"""Compares the Beads effect with the original Processor (hi-fi media,
deterministic settings: no randomization, regular grain clock)."""
import sys
import numpy as np
import bfx

BASE = dict(time=0.3, time_amt=0, size=0.7, size_amt=0, shape=0.55, shape_amt=0,
            pitch=0, pitch_amt=0, density=0.3, feedback=0, dry_wet=0.5, reverb=0,
            freeze=0, seed=2)


def port_params(cfg):
    p = {k: v for k, v in cfg.items() if k not in ("seed",)}
    p["latch"] = 1 if cfg["seed"] == 2 else 0
    p["seed"] = 1 if cfg["seed"] == 1 else 0
    return p


def compare(fx, x, name, media=3, seed_at=None, freeze_at=None, skip=0.05, **over):
    """seed_at: SEED taps (s); freeze_at: (on, off) times (s); skip: seconds
    left out of the error (e.g. a randomized granular lead-in)."""
    cfg = dict(BASE); cfg.update(over)
    pp = port_params(cfg); pp["quality"] = media
    r = bfx.ref(x, media=media, seed_at=seed_at, freeze_at=freeze_at, **cfg)
    ramps = {}
    if seed_at:
        blk = 12 / bfx.SR  # one module block, as the reference taps
        pts = [(0, 0)]
        for t in seed_at:
            pts += [(t, 1), (t + blk, 0)]
        ramps["seed"] = pts
    if freeze_at:
        ramps["freeze"] = [(0, 0), (freeze_at[0], 1), (freeze_at[1], 0)]
    p = fx.render_fx(x, params=pp, ramps=ramps, exact=True)
    n = min(len(r), len(p)); r, p = r[:n], p[:n]
    s = int(skip * bfx.SR)
    rr = np.sqrt((r[s:] ** 2).mean()); pr = np.sqrt((p[s:] ** 2).mean())
    err = np.sqrt(((p[s:] - r[s:]) ** 2).mean())
    print(f"{name:34s} ref {rr:.4f} port {pr:.4f} err/ref {err / max(rr, 1e-9):.4f}")
    return r, p


def main():
    fx = bfx.effect()
    x = bfx.test_signal(3.0)
    compare(fx, x, "dry only", dry_wet=0)
    compare(fx, x, "grains regular", dry_wet=1)
    compare(fx, x, "grains short decay", dry_wet=1, size=0.45, shape=0.2)
    compare(fx, x, "grains bell dense", dry_wet=1, density=0.05, shape=0.6)
    compare(fx, x, "grains reverse", dry_wet=1, size=0.2)
    compare(fx, x, "grains +7 st", dry_wet=1, pitch=7)
    compare(fx, x, "grains -12 st reverse", dry_wet=1, pitch=-12, size=0.25)
    compare(fx, x, "grains time 0.9", dry_wet=1, time=0.9)
    compare(fx, x, "grains feedback .6", dry_wet=1, feedback=0.6)
    compare(fx, x, "grains freeze 1-2 s", dry_wet=1, freeze_at=(1.0, 2.0))
    compare(fx, x, "grains freeze, reverse +5", dry_wet=1, size=0.25, pitch=5, freeze_at=(0.8, 2.2))
    compare(fx, x, "mix + reverb", dry_wet=0.5, reverb=0.7)
    compare(fx, x, "delay mode", size=1, dry_wet=1)
    compare(fx, x, "delay density .3", size=1, dry_wet=1, density=0.3)
    compare(fx, x, "delay density .7 (odd taps)", size=1, dry_wet=1, density=0.7, skip=1.6)
    compare(fx, x, "delay time .5 feedback .5", size=1, dry_wet=1, time=0.5, feedback=0.5)
    compare(fx, x, "delay +5 st", size=1, dry_wet=1, pitch=5)
    compare(fx, x, "delay LP filter", size=1, size_amt=-0.6, dry_wet=1, skip=1.6)
    compare(fx, x, "delay HP filter", size=1, size_amt=0.6, dry_wet=1, skip=1.6)
    compare(fx, x, "delay shape .1", size=1, shape=0.1, dry_wet=1)
    compare(fx, x, "delay frozen slices", size=1, dry_wet=1, freeze_at=(1.8, 2.8))
    compare(fx, x, "delay frozen slices shape .6", size=1, dry_wet=1, shape=0.6, time=0.1, freeze_at=(1.8, 2.8))
    compare(fx, x, "delay sync (unlatched)", size=1, seed=0, dry_wet=1, density=0.45)
    compare(fx, x, "delay sync, tapped", size=1, seed=0, dry_wet=1, density=0.55, seed_at=(1.6, 1.9, 2.2, 2.5))
    compare(fx, x, "seed taps (density noon)", dry_wet=1, seed=0, density=0.5, seed_at=(0.4, 0.9, 1.3, 2.0))


if __name__ == "__main__":
    main()
