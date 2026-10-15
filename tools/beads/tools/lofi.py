#!/usr/bin/env python3
"""Sanity checks for the lo-fi media against the original (not sample
matched: the port band-limits at the host rate instead of resampling)."""
import numpy as np


def resample_poly(y, up, down, axis=0):
    """Band-limited FFT resampling by up/down (test signals only)."""
    n = y.shape[0]
    m = n * up // down
    Y = np.fft.rfft(y, axis=0)
    k = min(Y.shape[0], m // 2 + 1)
    Z = np.zeros((m // 2 + 1,) + Y.shape[1:], dtype=complex)
    Z[:k] = Y[:k]
    return np.fft.irfft(Z, n=m, axis=0) * (m / n)
import bfx
import compare as C


def band_db(y, lo, hi):
    f = np.fft.rfftfreq(len(y), 1 / bfx.SR)
    s = np.abs(np.fft.rfft(y[:, 0] * np.hanning(len(y)))) ** 2
    return 10 * np.log10(s[(f >= lo) & (f < hi)].sum() + 1e-20)


def main():
    fx = bfx.effect()
    x = bfx.test_signal(3.0)
    for media, name in [(0, "dirty tape"), (1, "clean tape"), (2, "vintage"), (3, "hi-fi")]:
        for label, over in [("dry", dict(dry_wet=0)), ("grains", dict(dry_wet=1, time=0.03)),
                            ("grains fb", dict(dry_wet=1, feedback=0.7, time=0.03)),
                            ("delay", dict(dry_wet=1, size=1, time=0.05)), ("reverb", dict(dry_wet=0.5, reverb=0.8))]:
            cfg = dict(C.BASE); cfg.update(over)
            pp = C.port_params(cfg); pp["quality"] = media
            # The module's codec runs at 24 / 32 kHz for these two media.
            up, down = {0: (1, 2), 2: (2, 3)}.get(media, (1, 1))
            xr = resample_poly(x, up, down).astype(np.float32) if up != down else x
            r = bfx.ref(xr, media=media, **cfg)
            if up != down:
                r = resample_poly(r, down, up)
            p = fx.render_fx(x, params=pp)
            n = min(len(r), len(p)); r, p = r[2400:n], p[2400:n]
            rr, pr = np.sqrt((r ** 2).mean()), np.sqrt((p ** 2).mean())
            hf_r, hf_p = band_db(r, 12500, 24000) - band_db(r, 100, 4000), band_db(p, 12500, 24000) - band_db(p, 100, 4000)
            print(f"{name:10s} {label:9s} rms ref {rr:.4f} port {pr:.4f} ({20 * np.log10(max(pr, 1e-9) / max(rr, 1e-9)):+.1f} dB)"
                  f"  >12.5k rel ref {hf_r:6.1f} port {hf_p:6.1f} dB  finite {np.isfinite(p).all()}")


if __name__ == "__main__":
    main()
