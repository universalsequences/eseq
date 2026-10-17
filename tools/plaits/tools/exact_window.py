import numpy as np
def exact_report(t, label, **kw):
    """lag-0 relative error in the first 20/100 ms and the first sample where
    |port-ref| exceeds 1% of the channel rms (time of divergence)."""
    r = t.ref(**kw); p = t.port(**kw); n = min(len(r), len(p)); r, p = r[:n], p[:n]
    parts = []
    for ch, nm in ((0, "out"), (1, "aux")):
        rr = float(np.sqrt(np.mean(r[:, ch] ** 2)))
        d = np.abs(p[:, ch] - r[:, ch])
        e20 = float(np.sqrt(np.mean(d[:960] ** 2)) / max(np.sqrt(np.mean(r[:960, ch] ** 2)), 1e-9))
        e100 = float(np.sqrt(np.mean(d[:4800] ** 2)) / max(np.sqrt(np.mean(r[:4800, ch] ** 2)), 1e-9))
        bad = np.nonzero(d > 0.01 * rr)[0]
        div = bad[0] / 48 if len(bad) else n / 48
        parts.append(f"{nm}: rms {rr:.4f} e20ms {e20:.5f} e100ms {e100:.5f} diverge@{div:7.1f}ms")
    print(f"{label:24s} " + "  ".join(parts), flush=True)
